use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::STT_MIN_BATCH_BYTES;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::matrix::{
    default_stt_model, parse_v2_locator, require_v2_locator_for_streaming, stt_default_transport,
    stt_uses_v2_streaming, GoogleSttV2Locator, SttDefaultTransport,
};

#[cfg(feature = "live")]
use crate::stt_live;

pub struct GoogleStt {
    model: String,
    language: String,
    transport: SttDefaultTransport,
    v2_locator: Option<GoogleSttV2Locator>,
    state: Arc<Mutex<GoogleSttState>>,
    /// One REST client for the provider's life: connections (and TLS sessions) are reused.
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// The V2 gRPC client, built on first use and kept.
    #[cfg(feature = "live")]
    grpc: tokio::sync::OnceCell<google_cloud_speech_v2::client::Speech>,
    #[cfg(feature = "live")]
    rest_url_override: Option<String>,
    last_transport: Arc<std::sync::Mutex<Option<String>>>,
}

struct GoogleSttState {
    running: bool,
    buffered: Vec<u8>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    #[cfg(feature = "live")]
    v2_handle: Option<stt_live::V2StreamHandle>,
}

impl GoogleStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        crate::matrix::check_stt_transport(config.transport)?;
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_model().to_string());
        let transport = stt_default_transport(&model).map_err(SpeechError::Config)?;
        let v2_locator = parse_v2_locator(config);
        require_v2_locator_for_streaming(&model, &v2_locator).map_err(SpeechError::Config)?;
        Ok(Self {
            language: config
                .language
                .clone()
                .unwrap_or_else(|| "en-US".to_string()),
            model,
            transport,
            v2_locator,
            state: Arc::new(Mutex::new(GoogleSttState {
                running: false,
                buffered: Vec::new(),
                transcript_rx: None,
                #[cfg(feature = "live")]
                v2_handle: None,
            })),
            #[cfg(feature = "live")]
            http: crate::http::build_client(None)?,
            #[cfg(feature = "live")]
            grpc: tokio::sync::OnceCell::new(),
            #[cfg(feature = "live")]
            rest_url_override: None,
            last_transport: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// Test hook: V1 `speech:recognize` goes to this URL (plain `http://` mock server) with a
    /// fixed bearer token instead of Google credentials.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __with_rest_url_override(mut self, url: impl Into<String>) -> Self {
        self.rest_url_override = Some(url.into());
        self
    }

    /// Test hook: replaces the pooled REST client with one that has this whole-request bound.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __with_total_timeout(mut self, total: std::time::Duration) -> Self {
        self.http = crate::http::build_client(Some(total)).expect("test http client");
        self
    }
}

#[async_trait]
impl SttProvider for GoogleStt {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    /// Only V2 `StreamingRecognize` (interim results) emits partials; the V1 REST `recognize`
    /// path returns one final per utterance.
    fn emits_partials(&self) -> bool {
        cfg!(feature = "live") && stt_uses_v2_streaming(self.transport)
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    async fn start(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = true;
        state.buffered.clear();
        state.transcript_rx = None;
        #[cfg(feature = "live")]
        {
            state.v2_handle = None;
            if stt_uses_v2_streaming(self.transport) {
                let locator = self.v2_locator.clone().ok_or_else(|| {
                    SpeechError::Config("missing Google V2 recognizer configuration".into())
                })?;
                let client = self
                    .grpc
                    .get_or_try_init(stt_live::build_grpc_client)
                    .await?;
                let (rx, handle) = stt_live::start_v2_streaming(
                    client,
                    &self.last_transport,
                    &locator,
                    &self.model,
                    &self.language,
                )
                .await?;
                state.transcript_rx = Some(rx);
                state.v2_handle = Some(handle);
            }
        }
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        state.running = false;
        state.buffered.clear();
        state.transcript_rx = None;
        #[cfg(feature = "live")]
        {
            if let Some(handle) = state.v2_handle.take() {
                handle.stop().await;
            }
        }
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let mut state = self.state.lock().await;
        if !state.running {
            return Ok(());
        }
        #[cfg(feature = "live")]
        if stt_uses_v2_streaming(self.transport) {
            if let Some(handle) = &state.v2_handle {
                handle.push_audio(pcm).await?;
                return Ok(());
            }
        }
        state.buffered.extend_from_slice(pcm.as_ref());
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut state = self.state.lock().await;
        if !state.running {
            return Ok(None);
        }
        if let Some(rx) = state.transcript_rx.as_mut() {
            if let Ok(t) = rx.try_recv() {
                return Ok(Some(t));
            }
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        if !matches!(self.transport, SttDefaultTransport::V1RestRecognize) {
            #[cfg(feature = "live")]
            if stt_uses_v2_streaming(self.transport) {
                if let Some(handle) = {
                    let state = self.state.lock().await;
                    state.v2_handle.clone()
                } {
                    handle.finalize_stream().await?;
                    return Ok(());
                }
            }
            return Ok(());
        }

        #[cfg(feature = "live")]
        {
            let pcm = {
                let mut state = self.state.lock().await;
                if state.buffered.len() < STT_MIN_BATCH_BYTES {
                    return Ok(());
                }
                Bytes::from(std::mem::take(&mut state.buffered))
            };
            let language = self.language.clone();
            let http = self.http.clone();
            let transport = self.last_transport.clone();
            let url_override = self.rest_url_override.clone();
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let result = stt_live::v1_recognize(
                    &http,
                    &transport,
                    url_override.as_deref(),
                    pcm,
                    &language,
                )
                .await;
                if let Ok(text) = result {
                    if !text.trim().is_empty() {
                        let _ = tx.send(SttTranscript::Final(text));
                    }
                }
            });
            let mut state = self.state.lock().await;
            state.transcript_rx = Some(rx);
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            Err(SpeechError::Vendor {
                vendor: "google".into(),
                message: "live Google STT requires `--features live` on vendor-google".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    fn config(model: &str, endpoint: Option<&str>) -> SttConfig {
        SttConfig {
            provider: SttVendor::Google,
            model: Some(model.into()),
            model_path: None,
            language: Some("en-US".into()),
            api_key: None,
            endpoint: endpoint.map(str::to_string),
            transport: None,
        }
    }

    #[test]
    fn v1_rest_model_never_emits_partials() {
        let stt = GoogleStt::new(&config("latest_long", None)).unwrap();
        assert!(!stt.emits_partials());
    }

    #[test]
    fn v2_streaming_model_emits_partials_when_live() {
        let stt = GoogleStt::new(&config(
            "chirp_3",
            Some("projects/p/locations/l/recognizers/r"),
        ))
        .unwrap();
        assert_eq!(stt.emits_partials(), cfg!(feature = "live"));
    }

    #[test]
    fn chirp_requires_v2_locator() {
        assert!(matches!(
            GoogleStt::new(&SttConfig {
                provider: SttVendor::Google,
                model: Some("chirp_3".into()),
                model_path: None,
                language: Some("en-US".into()),
                api_key: None,
                endpoint: None,
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
