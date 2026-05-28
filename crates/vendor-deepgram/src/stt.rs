use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

#[cfg(feature = "live")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "live")]
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

pub struct DeepgramStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    state: Arc<Mutex<DeepgramSttInner>>,
}

struct DeepgramSttInner {
    running: bool,
    audio_tx: Option<mpsc::UnboundedSender<Bytes>>,
    transcript_rx: Option<mpsc::UnboundedReceiver<SttTranscript>>,
    reader_task: Option<tokio::task::JoinHandle<()>>,
    sender_task: Option<tokio::task::JoinHandle<()>>,
}

impl DeepgramStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok()),
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "nova-2".to_string()),
            language: config.language.clone(),
            state: Arc::new(Mutex::new(DeepgramSttInner {
                running: false,
                audio_tx: None,
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
            .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing DEEPGRAM_API_KEY".into()))
    }
}

#[async_trait]
impl SttProvider for DeepgramStt {
    fn vendor_name(&self) -> &'static str {
        "deepgram"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let model = self.model.clone();
            let language = self.language.clone();
            let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<Bytes>();
            let (transcript_tx, transcript_rx) = mpsc::unbounded_channel::<SttTranscript>();

            let mut url = format!(
                "wss://api.deepgram.com/v1/listen?model={}&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&punctuate=true",
                model
            );
            if let Some(language) = language {
                url.push_str("&language=");
                url.push_str(&language);
            }

            let mut request = url
                .into_client_request()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "deepgram".into(),
                    message: err.to_string(),
                })?;
            request.headers_mut().insert(
                "Authorization",
                format!("Token {api_key}")
                    .parse()
                    .map_err(|err| SpeechError::Vendor {
                        vendor: "deepgram".into(),
                        message: format!("invalid auth header: {err}"),
                    })?,
            );

            let (ws, _) = connect_async(request).await.map_err(|err| SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: err.to_string(),
            })?;
            let (mut ws_tx, mut ws_rx) = ws.split();

            let reader_task = tokio::spawn(async move {
                while let Some(msg) = ws_rx.next().await {
                    let Ok(msg) = msg else { break };
                    if let Message::Text(text) = msg {
                        if let Some(transcript) = parse_deepgram_message(&text) {
                            let _ = transcript_tx.send(transcript);
                        }
                    }
                }
            });

            let sender_task = tokio::spawn(async move {
                while let Some(chunk) = audio_rx.recv().await {
                    if ws_tx.send(Message::Binary(chunk)).await.is_err() {
                        break;
                    }
                }
                let _ = ws_tx.send(Message::Close(None)).await;
            });

            let mut inner = self.state.lock().await;
            inner.running = true;
            inner.audio_tx = Some(audio_tx);
            inner.transcript_rx = Some(transcript_rx);
            inner.reader_task = Some(reader_task);
            inner.sender_task = Some(sender_task);
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = self.api_key()?;
            Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "live Deepgram STT requires `--features live` on vendor-deepgram".into(),
            })
        }
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.state.lock().await;
        inner.running = false;
        inner.audio_tx = None;
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
        if let Some(tx) = &inner.audio_tx {
            let _ = tx.send(pcm);
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
}

#[cfg(feature = "live")]
fn parse_deepgram_message(raw: &str) -> Option<SttTranscript> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if value.get("type")?.as_str()? != "Results" {
        return None;
    }
    let transcript = value
        .pointer("/channel/alternatives/0/transcript")?
        .as_str()?
        .trim();
    if transcript.is_empty() {
        return None;
    }
    let is_final = value.get("is_final").and_then(|v| v.as_bool()).unwrap_or(true);
    if is_final {
        Some(SttTranscript::Final(transcript.to_string()))
    } else {
        Some(SttTranscript::Partial(transcript.to_string()))
    }
}

#[cfg(not(feature = "live"))]
fn parse_deepgram_message(_raw: &str) -> Option<SttTranscript> {
    None
}
