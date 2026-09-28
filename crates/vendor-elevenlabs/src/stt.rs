use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::stt_matrix::{
    default_stt_model, input_audio_chunk_json, parse_realtime_message, realtime_websocket_url,
    validate_stt_model, STT_PCM_SAMPLE_RATE,
};

#[cfg(feature = "live")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "live")]
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

enum OutboundMessage {
    Audio(Bytes),
    Commit,
}

pub struct ElevenLabsStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    state: Arc<Mutex<ElevenLabsSttInner>>,
}

struct ElevenLabsSttInner {
    running: bool,
    outbound_tx: Option<mpsc::UnboundedSender<OutboundMessage>>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
    sender_task: Option<tokio::task::JoinHandle<()>>,
}

impl ElevenLabsStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_stt_model().to_string());
        validate_stt_model(&model).map_err(SpeechError::Config)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok()),
            model,
            language: config.language.clone(),
            state: Arc::new(Mutex::new(ElevenLabsSttInner {
                running: false,
                outbound_tx: None,
                transcript_rx: None,
                reader_task: None,
                sender_task: None,
            })),
        })
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing ELEVENLABS_API_KEY".into()))
    }
}

#[async_trait]
impl SttProvider for ElevenLabsStt {
    fn vendor_name(&self) -> &'static str {
        "elevenlabs"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let url = realtime_websocket_url(&self.model, self.language.as_deref())
                .map_err(SpeechError::Config)?;

            let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<OutboundMessage>();
            let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttTranscript>();

            let mut request = url
                .into_client_request()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: err.to_string(),
                })?;
            request.headers_mut().insert(
                "xi-api-key",
                api_key.parse().map_err(|err| SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: format!("invalid xi-api-key header: {err}"),
                })?,
            );

            let (ws, _) = connect_async(request)
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: err.to_string(),
                })?;
            let (mut ws_tx, mut ws_rx) = ws.split();

            let reader_task = tokio::spawn(async move {
                while let Some(msg) = ws_rx.next().await {
                    let Ok(msg) = msg else { break };
                    if let Message::Text(text) = msg {
                        if let Some(transcript) = parse_realtime_message(&text) {
                            let _ = transcript_tx.send(transcript);
                        }
                    }
                }
            });

            let sender_task = tokio::spawn(async move {
                use base64::Engine;
                while let Some(outbound) = outbound_rx.recv().await {
                    let json = match outbound {
                        OutboundMessage::Audio(pcm) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(pcm);
                            input_audio_chunk_json(&b64, false)
                        }
                        OutboundMessage::Commit => input_audio_chunk_json("", true),
                    };
                    if ws_tx.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                let _ = ws_tx.send(Message::Close(None)).await;
            });

            let mut inner = self.state.lock().await;
            inner.running = true;
            inner.outbound_tx = Some(outbound_tx);
            inner.transcript_rx = Some(transcript_rx);
            inner.reader_task = Some(reader_task);
            inner.sender_task = Some(sender_task);
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = self.api_key()?;
            Err(SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message:
                    "live ElevenLabs STT requires `--features live` on vendor-elevenlabs".into(),
            })
        }
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.state.lock().await;
        inner.running = false;
        inner.outbound_tx = None;
        if let Some(task) = inner.reader_task.take() {
            task.abort();
        }
        if let Some(task) = inner.sender_task.take() {
            task.abort();
        }
        inner.transcript_rx = None;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let inner = self.state.lock().await;
        if !inner.running {
            return Ok(());
        }
        if let Some(tx) = &inner.outbound_tx {
            let _ = tx.send(OutboundMessage::Audio(pcm));
        }
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut inner = self.state.lock().await;
        if !inner.running {
            return Ok(None);
        }
        if let Some(rx) = inner.transcript_rx.as_mut() {
            return Ok(rx.try_recv().ok());
        }
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let inner = self.state.lock().await;
        if !inner.running {
            return Ok(());
        }
        if let Some(tx) = &inner.outbound_tx {
            let _ = tx.send(OutboundMessage::Commit);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::SttVendor;

    #[test]
    fn rejects_undocumented_model_at_new() {
        assert!(matches!(
            ElevenLabsStt::new(&SttConfig {
                provider: SttVendor::Elevenlabs,
                model: Some("scribe_v1".into()),
                model_path: None,
                language: None,
                api_key: Some("k".into()),
                endpoint: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }

    #[test]
    fn stt_sample_rate_matches_matrix() {
        assert_eq!(STT_PCM_SAMPLE_RATE, 16_000);
    }
}
