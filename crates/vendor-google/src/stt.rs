use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::STT_MIN_BATCH_BYTES;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::matrix::{
    default_stt_model, resolve_v2_locator, stt_default_transport, stt_uses_v2_streaming,
    GoogleSttV2Locator, SttDefaultTransport,
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
    /// Explicit `config.api_key`: REST paths only (`crate::auth`).
    #[cfg(feature = "live")]
    api_key: Option<String>,
    /// `GOOGLE_APPLICATION_CREDENTIALS` at construction: the only credential of the gRPC path.
    #[cfg(feature = "live")]
    credentials_file: Option<String>,
    /// Failure of the last V1 request, reported once by `poll_transcript` (named, no secrets).
    #[cfg(feature = "live")]
    rest_error: Arc<std::sync::Mutex<Option<String>>>,
    last_transport: Arc<std::sync::Mutex<Option<String>>>,
}

struct GoogleSttState {
    running: bool,
    buffered: Vec<u8>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    /// V2: the sender every stream of this provider reports to (one receiver across streams).
    #[cfg(feature = "live")]
    transcript_tx: Option<mpsc::UnboundedSender<SttTranscript>>,
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
        let v2_locator = resolve_v2_locator(&model, config.endpoint.as_deref(), |name| {
            std::env::var(name).ok()
        })
        .map_err(SpeechError::Config)?;
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
                transcript_tx: None,
                #[cfg(feature = "live")]
                v2_handle: None,
            })),
            #[cfg(feature = "live")]
            http: crate::http::build_client(None)?,
            #[cfg(feature = "live")]
            grpc: tokio::sync::OnceCell::new(),
            #[cfg(feature = "live")]
            rest_url_override: None,
            #[cfg(feature = "live")]
            api_key: crate::auth::clean(config.api_key.as_deref()),
            #[cfg(feature = "live")]
            credentials_file: crate::auth::credentials_file_from_env(),
            #[cfg(feature = "live")]
            rest_error: Arc::new(std::sync::Mutex::new(None)),
            last_transport: Arc::new(std::sync::Mutex::new(None)),
        })
    }

    /// Test hook: pretend `GOOGLE_APPLICATION_CREDENTIALS` is unset.
    #[cfg(feature = "live")]
    #[doc(hidden)]
    pub fn __without_service_account(mut self) -> Self {
        self.credentials_file = None;
        self
    }

    /// Opens one V2 stream for the next utterance; results go to the provider's channel.
    #[cfg(feature = "live")]
    async fn open_v2_stream(&self, state: &mut GoogleSttState) -> SpeechResult<()> {
        let locator = self.v2_locator.clone().ok_or_else(|| {
            SpeechError::Config("missing Google V2 recognizer configuration".into())
        })?;
        let tx = state
            .transcript_tx
            .clone()
            .ok_or_else(|| SpeechError::Stt("google stt: provider is not started".into()))?;
        if let Some(old) = state.v2_handle.take() {
            old.stop().await;
        }
        let client = self
            .grpc
            .get_or_try_init(|| {
                stt_live::build_grpc_client(self.credentials_file.as_deref(), &locator, &self.model)
            })
            .await?;
        state.v2_handle = Some(
            stt_live::start_v2_streaming(
                client,
                &self.last_transport,
                &locator,
                &self.model,
                &self.language,
                tx,
            )
            .await?,
        );
        Ok(())
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
        #[cfg(feature = "live")]
        {
            let mut state = self.state.lock().await;
            state.running = true;
            state.buffered.clear();
            if let Some(old) = state.v2_handle.take() {
                old.stop().await;
            }
            if stt_uses_v2_streaming(self.transport) {
                let (tx, rx) = mpsc::unbounded_channel();
                state.transcript_tx = Some(tx);
                state.transcript_rx = Some(rx);
                if let Err(err) = self.open_v2_stream(&mut state).await {
                    state.running = false;
                    return Err(err);
                }
            } else {
                state.transcript_rx = None;
            }
        }
        #[cfg(not(feature = "live"))]
        {
            let mut state = self.state.lock().await;
            state.running = true;
            state.buffered.clear();
            state.transcript_rx = None;
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
            state.transcript_tx = None;
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
            // The previous utterance's stream is spent after its finalize: the next utterance
            // opens a new one on the same client.
            if state.v2_handle.as_ref().is_none_or(|h| h.is_spent()) {
                self.open_v2_stream(&mut state).await?;
            }
            if let Some(handle) = &state.v2_handle {
                handle.push_audio(pcm).await?;
            }
            return Ok(());
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
        #[cfg(feature = "live")]
        {
            if let Some(handle) = &state.v2_handle {
                handle.take_error()?;
            }
            if let Some(message) = self.rest_error.lock().ok().and_then(|mut g| g.take()) {
                return Err(SpeechError::Stt(message));
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
                    handle
                        .finalize_stream(std::time::Duration::from_millis(self.finalize_wait_ms()))
                        .await?;
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
            let api_key = self.api_key.clone();
            let model = self.model.clone();
            let rest_error = self.rest_error.clone();
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let result = stt_live::v1_recognize(
                    &http,
                    &transport,
                    url_override.as_deref(),
                    api_key.as_deref(),
                    pcm,
                    &language,
                    &model,
                )
                .await;
                match result {
                    Ok(text) => {
                        if !text.trim().is_empty() {
                            let _ = tx.send(SttTranscript::Final(text));
                        }
                    }
                    Err(err) => {
                        if let Ok(mut slot) = rest_error.lock() {
                            *slot = Some(format!("google stt: speech:recognize failed: {err}"));
                        }
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
}
