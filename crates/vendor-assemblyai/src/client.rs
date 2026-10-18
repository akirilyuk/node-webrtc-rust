//! AssemblyAI streaming v3 WebSocket client.
//!
//! <https://www.assemblyai.com/docs/speech-to-text/streaming>
//!
//! Protocol basis (published docs): "Expect several partial updates before each `end_of_turn:
//! true`" (so `Turn` messages with `end_of_turn: false` are partials); client messages
//! `{"type": "ForceEndpoint"}` ("Manually force an endpoint in the transcription.") and
//! `{"type": "Terminate"}` ("Terminate finalizes the open turn"); sessions allow "a 3-hour
//! maximum session duration" and several turns, so one socket serves many utterances.

use bytes::Bytes;
use futures_util::StreamExt;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::SttTranscript;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;

use crate::matrix::{parse_turn_message, streaming_websocket_url_for, DISCONNECT_BOUND, WS_BOUND};
use crate::ws_connect::send_bounded;

pub struct AssemblyAiClient {
    api_key: Option<String>,
    speech_model: String,
    endpoint: Option<String>,
    inner: Arc<Mutex<AssemblyAiInner>>,
    /// `ws` once the streaming socket is open.
    last_transport: std::sync::Mutex<Option<String>>,
}

/// What the socket task sends: audio, or the documented end-of-turn request.
enum Command {
    Audio(Bytes),
    ForceEndpoint,
}

struct AssemblyAiInner {
    cmd_tx: Option<mpsc::UnboundedSender<Command>>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    ws_task: Option<tokio::task::JoinHandle<()>>,
}

impl AssemblyAiClient {
    pub fn new(
        api_key: Option<String>,
        speech_model: String,
        endpoint: Option<String>,
    ) -> SpeechResult<Self> {
        crate::matrix::validate_speech_model(&speech_model).map_err(SpeechError::Config)?;
        Ok(Self {
            api_key,
            speech_model,
            endpoint,
            inner: Arc::new(Mutex::new(AssemblyAiInner {
                cmd_tx: None,
                transcript_rx: None,
                ws_task: None,
            })),
            last_transport: std::sync::Mutex::new(None),
        })
    }

    /// `ws` once the streaming socket is open.
    pub fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
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
        let url = streaming_websocket_url_for(&self.speech_model, self.endpoint.as_deref())
            .map_err(SpeechError::Config)?;
        let ws = crate::ws_connect::connect(url, &api_key).await?;
        if let Ok(mut guard) = self.last_transport.lock() {
            *guard = Some("ws".into());
        }
        let (mut ws_tx, mut ws_rx) = ws.split();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Command>();
        let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttTranscript>();

        let ws_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = cmd_rx.recv() => {
                        let message = match command {
                            None => break,
                            Some(Command::Audio(chunk)) => Message::Binary(chunk),
                            Some(Command::ForceEndpoint) => {
                                Message::Text(r#"{"type":"ForceEndpoint"}"#.into())
                            }
                        };
                        // A send that fails or stays stuck ends the task; the closed command
                        // channel then makes the next push_audio report the failure.
                        if send_bounded(&mut ws_tx, message, WS_BOUND).await.is_err() {
                            break;
                        }
                    }
                    msg = ws_rx.next() => match msg {
                        None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                        Some(Ok(Message::Text(text))) => {
                            if let Some(transcript) = parse_turn_message(&text) {
                                let _ = transcript_tx.send(transcript);
                            }
                        }
                        // Ping/Pong are answered by tungstenite; they must not end the session.
                        Some(Ok(_)) => {}
                    }
                }
            }
            let _ = send_bounded(
                &mut ws_tx,
                Message::Text(r#"{"type":"Terminate"}"#.into()),
                WS_BOUND,
            )
            .await;
        });

        let mut inner = self.inner.lock().await;
        inner.cmd_tx = Some(cmd_tx);
        inner.transcript_rx = Some(transcript_rx);
        inner.ws_task = Some(ws_task);
        Ok(())
    }

    fn closed() -> SpeechError {
        SpeechError::Vendor {
            vendor: "assemblyai".into(),
            message: "assemblyai stt websocket closed".into(),
        }
    }

    pub async fn push_audio(&self, pcm: Bytes) -> SpeechResult<()> {
        let inner = self.inner.lock().await;
        if let Some(tx) = &inner.cmd_tx {
            tx.send(Command::Audio(pcm)).map_err(|_| Self::closed())?;
        }
        Ok(())
    }

    /// Documented `ForceEndpoint`: ends the open turn without closing the session.
    pub async fn force_endpoint(&self) -> SpeechResult<()> {
        let inner = self.inner.lock().await;
        if let Some(tx) = &inner.cmd_tx {
            tx.send(Command::ForceEndpoint)
                .map_err(|_| Self::closed())?;
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
        // Dropping the sender ends the task's loop, which then sends `Terminate`.
        inner.cmd_tx = None;
        if let Some(mut task) = inner.ws_task.take() {
            if tokio::time::timeout(DISCONNECT_BOUND, &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        inner.transcript_rx = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_speech_model() {
        assert!(AssemblyAiClient::new(Some("k".into()), "bad-model".into(), None).is_err());
    }
}
