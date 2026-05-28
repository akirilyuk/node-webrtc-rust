//! AssemblyAI realtime WebSocket client.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::SttTranscript;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

pub struct AssemblyAiClient {
    api_key: Option<String>,
    inner: Arc<Mutex<AssemblyAiInner>>,
}

struct AssemblyAiInner {
    audio_tx: Option<mpsc::UnboundedSender<Bytes>>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    ws_task: Option<tokio::task::JoinHandle<()>>,
}

impl AssemblyAiClient {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            inner: Arc::new(Mutex::new(AssemblyAiInner {
                audio_tx: None,
                transcript_rx: None,
                ws_task: None,
            })),
        }
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("ASSEMBLYAI_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing ASSEMBLYAI_API_KEY".into()))
    }

    pub async fn connect(&self) -> SpeechResult<()> {
        let api_key = self.api_key()?;
        let url = "wss://api.assemblyai.com/v2/realtime/ws?sample_rate=16000";
        let mut request = url.into_client_request().map_err(|err| SpeechError::Vendor {
            vendor: "assemblyai".into(),
            message: err.to_string(),
        })?;
        request.headers_mut().insert(
            "Authorization",
            api_key.parse().map_err(|err| SpeechError::Vendor {
                vendor: "assemblyai".into(),
                message: format!("invalid auth header: {err}"),
            })?,
        );

        let (ws, _) = connect_async(request).await.map_err(|err| SpeechError::Vendor {
            vendor: "assemblyai".into(),
            message: err.to_string(),
        })?;
        let (mut ws_tx, mut ws_rx) = ws.split();
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Bytes>();
        let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttTranscript>();

        let ws_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    audio = audio_rx.recv() => {
                        let Some(chunk) = audio else { break };
                        let payload = serde_json::json!({
                            "audio_data": STANDARD.encode(chunk.as_ref())
                        });
                        if ws_tx
                            .send(Message::Text(payload.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    msg = ws_rx.next() => {
                        let Some(Ok(Message::Text(text))) = msg else { break };
                        if let Some(transcript) = parse_assemblyai_message(&text) {
                            let _ = transcript_tx.send(transcript);
                        }
                    }
                }
            }
            let _ = ws_tx
                .send(Message::Text(r#"{"terminate_session": true}"#.into()))
                .await;
        });

        let mut inner = self.inner.lock().await;
        inner.audio_tx = Some(audio_tx);
        inner.transcript_rx = Some(transcript_rx);
        inner.ws_task = Some(ws_task);
        Ok(())
    }

    pub async fn push_audio(&self, pcm: Bytes) -> SpeechResult<()> {
        let inner = self.inner.lock().await;
        if let Some(tx) = &inner.audio_tx {
            let _ = tx.send(pcm);
        }
        Ok(())
    }

    pub async fn poll_transcript(&self) -> SpeechResult<Option<SttTranscript>> {
        let mut inner = self.inner.lock().await;
        if let Some(rx) = inner.transcript_rx.as_mut() {
            return Ok(rx.try_recv().ok());
        }
        Ok(None)
    }

    pub async fn disconnect(&self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.audio_tx = None;
        if let Some(task) = inner.ws_task.take() {
            task.abort();
        }
        inner.transcript_rx = None;
        Ok(())
    }
}

fn parse_assemblyai_message(raw: &str) -> Option<SttTranscript> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let message_type = value.get("message_type")?.as_str()?;
    let text = value.get("text")?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    match message_type {
        "PartialTranscript" => Some(SttTranscript::Partial(text.to_string())),
        "FinalTranscript" => Some(SttTranscript::Final(text.to_string())),
        _ => None,
    }
}
