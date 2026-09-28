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
        })
    }
}

#[async_trait]
impl SttProvider for GoogleStt {
    fn vendor_name(&self) -> &'static str {
        "google"
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
                let locator = self.v2_locator.clone().ok_or_else(|| SpeechError::Config(
                    "missing Google V2 recognizer configuration".into(),
                ))?;
                let (rx, handle) = stt_live::start_v2_streaming(
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
            if let Some(t) = rx.try_recv().ok() {
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
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                let result = stt_live::v1_recognize(pcm, &language).await;
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
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
