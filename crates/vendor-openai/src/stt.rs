//! OpenAI STT over the Realtime transcription websocket.
//!
//! Audio is appended as it arrives (`input_audio_buffer.append`). The agent's
//! VAD owns the utterance: turn detection stays off, and `finalize_utterance`
//! sends one `input_audio_buffer.commit`. A 1-second HTTP flush, or OpenAI
//! server VAD, would close the transcript in the middle of a counting phrase.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, STT_MIN_BATCH_BYTES, STT_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use serde_json::Value;
use tokio::sync::{mpsc, Mutex};

#[cfg(feature = "live")]
use base64::Engine;
#[cfg(feature = "live")]
use futures_util::{SinkExt, StreamExt};
#[cfg(feature = "live")]
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

const OPENAI_REALTIME_URL: &str = "wss://api.openai.com/v1/realtime?intent=transcription";
const OPENAI_PCM_RATE: u32 = 24_000;

pub struct OpenAiStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    resampler: Pcm16kTo24k,
    inner: Arc<Mutex<StreamInner>>,
    /// Audio accepted since the last commit. Reported while a commit is in flight.
    live_backlog_ms: AtomicU32,
    frozen_backlog_ms: AtomicU32,
    commit_waiting: AtomicBool,
    uncommitted_bytes: usize,
}

struct StreamInner {
    running: bool,
    tx: Option<mpsc::UnboundedSender<ClientEvent>>,
    rx: Option<mpsc::UnboundedReceiver<Incoming>>,
    reader: Option<tokio::task::JoinHandle<()>>,
    writer: Option<tokio::task::JoinHandle<()>>,
}

enum ClientEvent {
    Append(Vec<u8>),
    Commit,
    Clear,
}

enum Incoming {
    Update(SttTranscript),
    Failed(String),
}

impl OpenAiStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());
        Ok(Self {
            api_key,
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "whisper-1".to_string()),
            language: config.language.clone(),
            resampler: Pcm16kTo24k::default(),
            inner: Arc::new(Mutex::new(StreamInner {
                running: false,
                tx: None,
                rx: None,
                reader: None,
                writer: None,
            })),
            live_backlog_ms: AtomicU32::new(0),
            frozen_backlog_ms: AtomicU32::new(0),
            commit_waiting: AtomicBool::new(false),
            uncommitted_bytes: 0,
        })
    }

    fn note_pushed_audio(&self, pcm_len: usize) {
        if pcm_len == 0 {
            return;
        }
        let ms = duration_ms_from_mono_s16le(pcm_len, STT_PCM_SAMPLE_RATE);
        self.live_backlog_ms.fetch_add(ms, Ordering::Relaxed);
    }

    fn freeze_backlog_for_commit(&self) {
        let utterance_ms = self.live_backlog_ms.swap(0, Ordering::Relaxed);
        self.frozen_backlog_ms
            .store(utterance_ms, Ordering::Relaxed);
        self.commit_waiting.store(true, Ordering::Relaxed);
    }

    fn clear_backlog(&self) {
        self.live_backlog_ms.store(0, Ordering::Relaxed);
        self.frozen_backlog_ms.store(0, Ordering::Relaxed);
        self.commit_waiting.store(false, Ordering::Relaxed);
    }

    fn on_final_delivered(&self) {
        self.commit_waiting.store(false, Ordering::Relaxed);
        self.frozen_backlog_ms.store(0, Ordering::Relaxed);
    }

    async fn send_event(&self, event: ClientEvent) -> SpeechResult<()> {
        let inner = self.inner.lock().await;
        if !inner.running {
            return Ok(());
        }
        if let Some(tx) = &inner.tx {
            tx.send(event).map_err(|_| SpeechError::Vendor {
                vendor: "openai".into(),
                message: "realtime transcription socket closed".into(),
            })?;
        }
        Ok(())
    }
}

/// 16 kHz mono s16le → 24 kHz, which is what the Realtime transcription socket requires.
#[derive(Default)]
pub(crate) struct Pcm16kTo24k {
    src: Vec<i16>,
    src_dropped: u64,
    next_out: u64,
}

impl Pcm16kTo24k {
    pub(crate) fn push_s16le(&mut self, pcm: &[u8]) -> Vec<u8> {
        let mut index = 0;
        while index + 1 < pcm.len() {
            self.src
                .push(i16::from_le_bytes([pcm[index], pcm[index + 1]]));
            index += 2;
        }
        self.emit(false)
    }

    /// Flush samples that were waiting on the next input frame.
    pub(crate) fn finish(&mut self) -> Vec<u8> {
        let tail = self.emit(true);
        self.src.clear();
        self.src_dropped = 0;
        self.next_out = 0;
        tail
    }

    fn emit(&mut self, finished: bool) -> Vec<u8> {
        let total_src = self.src_dropped + self.src.len() as u64;
        if total_src == 0 {
            return Vec::new();
        }
        let max_out = if finished {
            (total_src * u64::from(OPENAI_PCM_RATE)) / u64::from(STT_PCM_SAMPLE_RATE)
        } else if total_src < 2 {
            return Vec::new();
        } else {
            ((total_src - 1) * u64::from(OPENAI_PCM_RATE)) / u64::from(STT_PCM_SAMPLE_RATE)
        };
        if self.next_out >= max_out {
            return Vec::new();
        }

        let mut samples = Vec::with_capacity((max_out - self.next_out) as usize);
        while self.next_out < max_out {
            let src_pos =
                self.next_out as f64 * f64::from(STT_PCM_SAMPLE_RATE) / f64::from(OPENAI_PCM_RATE);
            let abs_left = src_pos.floor() as u64;
            let local_left = abs_left.saturating_sub(self.src_dropped) as usize;
            if local_left >= self.src.len() {
                break;
            }
            let local_right = (local_left + 1).min(self.src.len() - 1);
            let frac = (src_pos - abs_left as f64) as f32;
            let left = f32::from(self.src[local_left]);
            let right = f32::from(self.src[local_right]);
            let sample = left * (1.0 - frac) + right * frac;
            samples.push(sample as i16);
            self.next_out += 1;
        }

        let consumed = (self.next_out as f64 * f64::from(STT_PCM_SAMPLE_RATE)
            / f64::from(OPENAI_PCM_RATE))
        .floor() as u64;
        if consumed > self.src_dropped {
            let drop = ((consumed - self.src_dropped) as usize).min(self.src.len());
            // Keep the sample the next output still interpolates against.
            let drop = drop.saturating_sub(1);
            if drop > 0 {
                self.src.drain(..drop);
                self.src_dropped += drop as u64;
            }
        }

        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }
}

/// Accumulates realtime deltas into the full hypothesis. Completion wins over the delta buffer.
#[derive(Default)]
pub(crate) struct TranscriptAssembler {
    item_id: String,
    text: String,
}

impl TranscriptAssembler {
    pub(crate) fn push(&mut self, event: &Value) -> Option<SttTranscript> {
        match event.get("type").and_then(Value::as_str) {
            Some("conversation.item.input_audio_transcription.delta") => {
                let item_id = event.get("item_id").and_then(Value::as_str).unwrap_or("");
                if item_id != self.item_id {
                    self.item_id = item_id.to_string();
                    self.text.clear();
                }
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                }
                let text = self.text.trim();
                if text.is_empty() {
                    None
                } else {
                    Some(SttTranscript::Partial(text.to_string()))
                }
            }
            Some("conversation.item.input_audio_transcription.completed") => {
                let transcript = event
                    .get("transcript")
                    .and_then(Value::as_str)
                    .unwrap_or(self.text.as_str())
                    .trim()
                    .to_string();
                self.text.clear();
                self.item_id.clear();
                Some(SttTranscript::Final(transcript))
            }
            Some("error") => None,
            _ => None,
        }
    }
}

pub(crate) fn session_update_body(model: &str, language: Option<&str>) -> Value {
    let mut transcription = serde_json::json!({ "model": model });
    if let Some(language) = language {
        if model.contains("live-transcribe") {
            transcription["languages"] = serde_json::json!([language]);
        } else {
            transcription["language"] = serde_json::json!(language);
        }
    }
    serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": OPENAI_PCM_RATE },
                    "transcription": transcription,
                    "turn_detection": null
                }
            }
        }
    })
}

fn realtime_error_message(event: &Value) -> Option<String> {
    if event.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    Some(
        event
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| event.get("message").and_then(Value::as_str))
            .unwrap_or("openai realtime error")
            .to_string(),
    )
}

#[async_trait]
impl SttProvider for OpenAiStt {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    fn decode_backlog_ms(&self) -> u32 {
        if self.commit_waiting.load(Ordering::Relaxed) {
            self.frozen_backlog_ms.load(Ordering::Relaxed)
        } else {
            self.live_backlog_ms.load(Ordering::Relaxed)
        }
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.stop().await?;
        #[cfg(feature = "live")]
        {
            return self.start_realtime().await;
        }
        #[cfg(not(feature = "live"))]
        {
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI STT requires `--features live` on vendor-openai".into(),
            })
        }
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.running = false;
        inner.tx = None;
        inner.rx = None;
        if let Some(task) = inner.reader.take() {
            task.abort();
        }
        if let Some(task) = inner.writer.take() {
            task.abort();
        }
        self.resampler = Pcm16kTo24k::default();
        self.uncommitted_bytes = 0;
        self.clear_backlog();
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let inner = self.inner.lock().await;
        if !inner.running {
            return Ok(());
        }
        drop(inner);
        if pcm.is_empty() {
            return Ok(());
        }
        self.uncommitted_bytes = self.uncommitted_bytes.saturating_add(pcm.len());
        self.note_pushed_audio(pcm.len());
        let pcm24 = self.resampler.push_s16le(pcm.as_ref());
        if pcm24.is_empty() {
            return Ok(());
        }
        self.send_event(ClientEvent::Append(pcm24)).await
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut inner = self.inner.lock().await;
        let Some(rx) = inner.rx.as_mut() else {
            return Ok(None);
        };
        match rx.try_recv() {
            Ok(Incoming::Update(transcript)) => {
                let is_final = matches!(transcript, SttTranscript::Final(_));
                drop(inner);
                if is_final {
                    self.on_final_delivered();
                }
                Ok(Some(transcript))
            }
            Ok(Incoming::Failed(message)) => Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message,
            }),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "realtime transcription socket closed".into(),
            }),
        }
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        let running = self.inner.lock().await.running;
        if !running {
            return Ok(());
        }
        let tail = self.resampler.finish();
        if !tail.is_empty() {
            self.send_event(ClientEvent::Append(tail)).await?;
        }
        if self.uncommitted_bytes < STT_MIN_BATCH_BYTES {
            self.uncommitted_bytes = 0;
            self.clear_backlog();
            return self.send_event(ClientEvent::Clear).await;
        }
        self.uncommitted_bytes = 0;
        self.freeze_backlog_for_commit();
        self.send_event(ClientEvent::Commit).await
    }
}

#[cfg(feature = "live")]
impl OpenAiStt {
    async fn start_realtime(&mut self) -> SpeechResult<()> {
        let api_key = crate::factory::api_key_from(&self.api_key, "OPENAI_API_KEY")?;
        let mut request =
            OPENAI_REALTIME_URL
                .into_client_request()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: err.to_string(),
                })?;
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {api_key}")
                .parse()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: format!("invalid auth header: {err}"),
                })?,
        );

        let (ws, _) = connect_async(request)
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "openai".into(),
                message: err.to_string(),
            })?;
        let (mut sink, mut stream) = ws.split();
        let session = session_update_body(&self.model, self.language.as_deref());
        sink.send(Message::Text(session.to_string().into()))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "openai".into(),
                message: err.to_string(),
            })?;

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            wait_until_session_ready(&mut stream),
        )
        .await
        .map_err(|_| SpeechError::Vendor {
            vendor: "openai".into(),
            message: "realtime transcription session was not ready".into(),
        })??;

        let (audio_tx, audio_rx) = mpsc::unbounded_channel();
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();

        let reader = tokio::spawn(async move {
            let mut assembler = TranscriptAssembler::default();
            while let Some(message) = stream.next().await {
                let Ok(message) = message else { break };
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(event) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(message) = realtime_error_message(&event) {
                    let _ = incoming_tx.send(Incoming::Failed(message));
                    break;
                }
                if let Some(transcript) = assembler.push(&event) {
                    if incoming_tx.send(Incoming::Update(transcript)).is_err() {
                        break;
                    }
                }
            }
        });

        let writer = tokio::spawn(async move {
            let mut audio_rx = audio_rx;
            while let Some(event) = audio_rx.recv().await {
                let payload = match event {
                    ClientEvent::Append(pcm) => serde_json::json!({
                        "type": "input_audio_buffer.append",
                        "audio": base64::engine::general_purpose::STANDARD.encode(pcm),
                    }),
                    ClientEvent::Commit => serde_json::json!({
                        "type": "input_audio_buffer.commit"
                    }),
                    ClientEvent::Clear => serde_json::json!({
                        "type": "input_audio_buffer.clear"
                    }),
                };
                if sink
                    .send(Message::Text(payload.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            let _ = sink.send(Message::Close(None)).await;
        });

        let mut inner = self.inner.lock().await;
        inner.running = true;
        inner.tx = Some(audio_tx);
        inner.rx = Some(incoming_rx);
        inner.reader = Some(reader);
        inner.writer = Some(writer);
        self.uncommitted_bytes = 0;
        self.clear_backlog();
        Ok(())
    }
}

#[cfg(feature = "live")]
async fn wait_until_session_ready<S>(stream: &mut S) -> SpeechResult<()>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(message) = stream.next().await {
        let message = message.map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;
        let Message::Text(text) = message else {
            continue;
        };
        let event: Value = serde_json::from_str(&text).map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;
        if let Some(message) = realtime_error_message(&event) {
            return Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message,
            });
        }
        if let Some("session.updated" | "transcription_session.updated") =
            event.get("type").and_then(Value::as_str)
        {
            return Ok(());
        }
    }
    Err(SpeechError::Vendor {
        vendor: "openai".into(),
        message: "realtime transcription socket closed before the session was ready".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oneshot_24k(pcm16: &[u8]) -> Vec<u8> {
        let mut resampler = Pcm16kTo24k::default();
        let mut out = resampler.push_s16le(pcm16);
        out.extend(resampler.finish());
        out
    }

    #[test]
    fn resample_streams_in_chunks_instead_of_one_buffer() {
        let pcm: Vec<u8> = (0..8_000u16).flat_map(|n| n.to_le_bytes()).collect();
        let mut resampler = Pcm16kTo24k::default();
        let mut chunked = Vec::new();
        let mut appends = 0usize;
        for piece in pcm.chunks(640) {
            let pcm24 = resampler.push_s16le(piece);
            if !pcm24.is_empty() {
                appends += 1;
                assert!(
                    pcm24.len() < pcm.len(),
                    "one append must be a slice of the utterance, not the whole buffer"
                );
                chunked.extend(pcm24);
            }
        }
        chunked.extend(resampler.finish());
        assert!(appends > 1);
        assert_eq!(chunked, oneshot_24k(&pcm));
        assert_eq!(chunked.len(), pcm.len() / 2 * 3);
    }

    #[test]
    fn session_update_commits_on_our_vad_not_server_vad() {
        let body = session_update_body("gpt-4o-mini-transcribe", Some("en"));
        assert_eq!(body["type"], "session.update");
        assert_eq!(body["session"]["type"], "transcription");
        assert!(body["session"]["audio"]["input"]["turn_detection"].is_null());
        assert_eq!(
            body["session"]["audio"]["input"]["transcription"]["model"],
            "gpt-4o-mini-transcribe"
        );
        assert_eq!(
            body["session"]["audio"]["input"]["transcription"]["language"],
            "en"
        );
        assert_eq!(body["session"]["audio"]["input"]["format"]["rate"], 24_000);
    }

    #[test]
    fn deltas_accumulate_and_completion_is_the_whole_utterance() {
        let mut assembler = TranscriptAssembler::default();
        let partial = assembler
            .push(&serde_json::json!({
                "type": "conversation.item.input_audio_transcription.delta",
                "item_id": "item_1",
                "delta": "one. two. "
            }))
            .expect("partial");
        assert_eq!(partial, SttTranscript::Partial("one. two.".into()));
        let grown = assembler
            .push(&serde_json::json!({
                "type": "conversation.item.input_audio_transcription.delta",
                "item_id": "item_1",
                "delta": "three."
            }))
            .expect("grown");
        assert_eq!(grown, SttTranscript::Partial("one. two. three.".into()));
        let final_text = assembler
            .push(&serde_json::json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "item_id": "item_1",
                "transcript": "one. two. three. four. five. six. seven. eight. nine. ten."
            }))
            .expect("final");
        assert_eq!(
            final_text,
            SttTranscript::Final(
                "one. two. three. four. five. six. seven. eight. nine. ten.".into()
            )
        );
    }

    #[test]
    fn error_event_is_a_failure_message() {
        let message = realtime_error_message(&serde_json::json!({
            "type": "error",
            "error": { "message": "buffer too small" }
        }));
        assert_eq!(message.as_deref(), Some("buffer too small"));
    }

    #[cfg(not(feature = "live"))]
    #[tokio::test]
    async fn without_live_feature_start_does_not_open_a_socket() {
        let mut stt = OpenAiStt::new(&node_webrtc_rust_speech::config::SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::Openai,
            model: Some("gpt-4o-mini-transcribe".into()),
            model_path: None,
            language: Some("en".into()),
            api_key: Some("test-key".into()),
            endpoint: None,
        })
        .expect("stt");
        let err = stt.start().await.expect_err("no socket");
        let SpeechError::Vendor { message, .. } = err else {
            panic!("expected vendor error");
        };
        assert!(message.contains("live"));
    }
}
