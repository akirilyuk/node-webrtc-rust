//! OpenAI STT per published docs:
//! <https://developers.openai.com/api/docs/guides/speech-to-text> (file JSON + file SSE),
//! <https://developers.openai.com/api/docs/guides/realtime-transcription> (WS `session.type=transcription`),
//! <https://developers.openai.com/api/docs/guides/realtime-websocket> (`wss://…/realtime?model=…`).
//!
//! HTTP models buffer 16 kHz PCM while the VAD gate is open; one upload runs on
//! [`SttProvider::finalize_utterance`]. [`SttProvider::poll_transcript`] drains JSON/SSE/Realtime
//! events and does not return `None` while a file POST or post-commit transcription is in flight.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono16_le_to_wav, STT_MIN_BATCH_BYTES, STT_PCM_SAMPLE_RATE,
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

use crate::matrix::{
    realtime_websocket_url, stt_default_transport, stt_file_sse_supported, SttDefaultTransport,
};

const OPENAI_PCM_RATE: u32 = 24_000;
const DEFAULT_OPENAI_API_BASE: &str = "https://api.openai.com/v1";

/// Legacy test helper — only `gpt-live-transcribe` uses Realtime by default in VoiceAgent.
pub(crate) fn model_uses_realtime_transcription(model: &str) -> bool {
    matches!(
        stt_default_transport(model),
        Ok(SttDefaultTransport::RealtimeLive)
    )
}

pub(crate) fn normalize_openai_api_base(endpoint: Option<&str>) -> String {
    match endpoint {
        Some(base) if !base.is_empty() => base.trim_end_matches('/').to_string(),
        _ => DEFAULT_OPENAI_API_BASE.to_string(),
    }
}

pub(crate) fn transcriptions_url(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if base.ends_with("/audio/transcriptions") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/audio/transcriptions")
    } else {
        format!("{base}/v1/audio/transcriptions")
    }
}

pub struct OpenAiStt {
    api_key: Option<String>,
    model: String,
    language: Option<String>,
    transport: Transport,
    /// When set, force Realtime WS (committed-turn / live tests) instead of the file default.
    force_realtime: bool,
}

enum Transport {
    Http(HttpBackend),
    Realtime(RealtimeBackend),
}

struct HttpBackend {
    api_base: String,
    file_sse: bool,
    inner: Mutex<HttpInner>,
    poll_rx: Mutex<Option<mpsc::UnboundedReceiver<SttTranscript>>>,
    poll_task: Mutex<Option<tokio::task::JoinHandle<SpeechResult<()>>>>,
    live_backlog_ms: AtomicU32,
    frozen_backlog_ms: AtomicU32,
    commit_waiting: AtomicBool,
}

struct HttpInner {
    running: bool,
    buffered: Vec<u8>,
}

struct RealtimeBackend {
    resampler: Pcm16kTo24k,
    inner: Arc<Mutex<StreamInner>>,
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
        Self::new_inner(config, false, None)
    }

    /// Realtime WS for `gpt-transcribe` committed-turn live tests (not the VoiceAgent default).
    #[cfg(feature = "live")]
    pub fn new_force_realtime(config: &SttConfig) -> SpeechResult<Self> {
        Self::new_inner(config, true, None)
    }

    /// Force file HTTP with `stream=true` off (live matrix: mini-transcribe JSON path).
    #[cfg(feature = "live")]
    pub fn new_force_http_file_json(config: &SttConfig) -> SpeechResult<Self> {
        Self::new_inner(config, false, Some(false))
    }

    fn new_inner(
        config: &SttConfig,
        force_realtime: bool,
        force_http_file_sse: Option<bool>,
    ) -> SpeechResult<Self> {
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| "whisper-1".to_string());
        let api_base = normalize_openai_api_base(config.endpoint.as_deref());
        let default = stt_default_transport(&model).map_err(|message| SpeechError::Vendor {
            vendor: "openai".into(),
            message,
        })?;
        let transport = if force_realtime {
            Transport::Realtime(RealtimeBackend::new())
        } else if let Some(file_sse) = force_http_file_sse {
            match default {
                SttDefaultTransport::RealtimeLive => {
                    return Err(SpeechError::Vendor {
                        vendor: "openai".into(),
                        message: "file HTTP override does not apply to Realtime STT models".into(),
                    });
                }
                SttDefaultTransport::FileJson | SttDefaultTransport::FileSse => {
                    Transport::Http(HttpBackend::new(api_base, file_sse))
                }
            }
        } else {
            match default {
                SttDefaultTransport::RealtimeLive => {
                    Transport::Realtime(RealtimeBackend::new())
                }
                SttDefaultTransport::FileJson => {
                    Transport::Http(HttpBackend::new(api_base, false))
                }
                SttDefaultTransport::FileSse => {
                    let sse = stt_file_sse_supported(&model);
                    Transport::Http(HttpBackend::new(api_base, sse))
                }
            }
        };
        Ok(Self {
            api_key,
            model,
            language: config.language.clone(),
            transport,
            force_realtime,
        })
    }
}

impl HttpBackend {
    fn new(api_base: String, file_sse: bool) -> Self {
        Self {
            api_base,
            file_sse,
            inner: Mutex::new(HttpInner {
                running: false,
                buffered: Vec::new(),
            }),
            poll_rx: Mutex::new(None),
            poll_task: Mutex::new(None),
            live_backlog_ms: AtomicU32::new(0),
            frozen_backlog_ms: AtomicU32::new(0),
            commit_waiting: AtomicBool::new(false),
        }
    }

    async fn clear_in_flight(&self) {
        if let Some(task) = self.poll_task.lock().await.take() {
            task.abort();
        }
        *self.poll_rx.lock().await = None;
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

    fn decode_backlog_ms(&self) -> u32 {
        if self.commit_waiting.load(Ordering::Relaxed) {
            self.frozen_backlog_ms.load(Ordering::Relaxed)
        } else {
            self.live_backlog_ms.load(Ordering::Relaxed)
        }
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.stop().await?;
        let mut inner = self.inner.lock().await;
        inner.running = true;
        inner.buffered.clear();
        self.clear_in_flight().await;
        self.clear_backlog();
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        inner.running = false;
        inner.buffered.clear();
        self.clear_in_flight().await;
        self.clear_backlog();
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        let mut inner = self.inner.lock().await;
        if !inner.running || pcm.is_empty() {
            return Ok(());
        }
        inner.buffered.extend_from_slice(pcm.as_ref());
        drop(inner);
        self.note_pushed_audio(pcm.len());
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        let mut rx_guard = self.poll_rx.lock().await;
        let Some(rx) = rx_guard.as_mut() else {
            return Ok(None);
        };
        if let Ok(transcript) = rx.try_recv() {
            let is_final = matches!(&transcript, SttTranscript::Final(_));
            if is_final {
                *rx_guard = None;
                self.poll_task.lock().await.take();
                self.on_final_delivered();
            }
            return Ok(Some(transcript));
        }
        let task_finished = self
            .poll_task
            .lock()
            .await
            .as_ref()
            .is_none_or(|task| task.is_finished());
        if task_finished {
            *rx_guard = None;
            if let Some(task) = self.poll_task.lock().await.take() {
                match task.await {
                    Ok(inner) => inner?,
                    Err(_) => {
                        return Err(SpeechError::Vendor {
                            vendor: "openai".into(),
                            message: "transcription task aborted".into(),
                        });
                    }
                }
            }
            self.on_final_delivered();
            return Ok(None);
        }
        // In flight: wait for the next documented event instead of returning None.
        match rx.recv().await {
            Some(transcript) => {
                let is_final = matches!(&transcript, SttTranscript::Final(_));
                if is_final {
                    *rx_guard = None;
                    self.poll_task.lock().await.take();
                    self.on_final_delivered();
                }
                Ok(Some(transcript))
            }
            None => {
                *rx_guard = None;
                self.poll_task.lock().await.take();
                self.on_final_delivered();
                Ok(None)
            }
        }
    }

    async fn finalize_utterance(
        &mut self,
        api_key: &Option<String>,
        model: &str,
        language: &Option<String>,
    ) -> SpeechResult<()> {
        if self.poll_rx.lock().await.is_some() {
            return Ok(());
        }

        let pcm = {
            let mut inner = self.inner.lock().await;
            if !inner.running {
                return Ok(());
            }
            std::mem::take(&mut inner.buffered)
        };

        if pcm.len() < STT_MIN_BATCH_BYTES {
            self.clear_backlog();
            return Ok(());
        }

        self.freeze_backlog_for_commit();

        #[cfg(feature = "live")]
        {
            let api_key = crate::factory::api_key_from(api_key, "OPENAI_API_KEY")?;
            let url = transcriptions_url(&self.api_base);
            let model = model.to_string();
            let language = language.clone();
            let pcm = Bytes::from(pcm);
            let use_sse = self.file_sse;
            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(async move {
                if use_sse {
                    http_transcribe_sse(pcm, &api_key, &url, &model, language.as_deref(), tx)
                        .await
                } else {
                    match http_transcribe(pcm, &api_key, &url, &model, language.as_deref()).await? {
                        Some(transcript) => {
                            let _ = tx.send(transcript);
                        }
                        None => {}
                    }
                    Ok(())
                }
            });
            *self.poll_rx.lock().await = Some(rx);
            *self.poll_task.lock().await = Some(handle);
            return Ok(());
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (api_key, model, language, pcm);
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI STT requires `--features live` on vendor-openai".into(),
            })
        }
    }
}

impl RealtimeBackend {
    fn new() -> Self {
        Self {
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
        }
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

    fn decode_backlog_ms(&self) -> u32 {
        if self.commit_waiting.load(Ordering::Relaxed) {
            self.frozen_backlog_ms.load(Ordering::Relaxed)
        } else {
            self.live_backlog_ms.load(Ordering::Relaxed)
        }
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
                if transcript.is_empty() {
                    None
                } else {
                    Some(SttTranscript::Final(transcript))
                }
            }
            Some("error") => None,
            _ => None,
        }
    }
}

/// OpenAPI `transcription_session.update` on a Realtime WebSocket (`?model=` handshake).
pub(crate) fn transcription_session_update_body(model: &str, language: Option<&str>) -> Value {
    let mut input_audio_transcription = serde_json::json!({ "model": model });
    if let Some(language) = language {
        input_audio_transcription["language"] = serde_json::json!(language);
    }
    serde_json::json!({
        "type": "transcription_session.update",
        "session": {
            "input_audio_format": "pcm16",
            "input_audio_transcription": input_audio_transcription,
            "turn_detection": null
        }
    })
}

/// Realtime transcription guide `session.update` with `session.type=transcription`.
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

#[cfg(feature = "live")]
async fn http_transcribe(
    pcm: Bytes,
    api_key: &str,
    transcriptions_url: &str,
    model: &str,
    language: Option<&str>,
) -> SpeechResult<Option<SttTranscript>> {
    use async_openai::types::{AudioInput, CreateTranscriptionRequestArgs, InputSource};

    let wav = mono16_le_to_wav(pcm.as_ref());
    let mut builder = CreateTranscriptionRequestArgs::default();
    builder.model(model);
    if let Some(language) = language {
        builder.language(language);
    }
    let request = builder
        .file(AudioInput {
            source: InputSource::Bytes {
                filename: "audio.wav".into(),
                bytes: wav.into(),
            },
        })
        .build()
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;

    let api_base = transcriptions_url
        .trim_end_matches("/audio/transcriptions")
        .trim_end_matches('/');
    let client = async_openai::Client::with_config(
        async_openai::config::OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(api_base),
    );
    let response = client
        .audio()
        .transcribe(request)
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;

    let text = response.text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(SttTranscript::Final(text.to_string())))
}

#[cfg(feature = "live")]
fn push_file_sse_event(event: &Value, tx: &mpsc::UnboundedSender<SttTranscript>) {
    match event.get("type").and_then(Value::as_str) {
        Some("transcript.text.delta") => {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                let text = delta.trim();
                if !text.is_empty() {
                    let _ = tx.send(SttTranscript::Partial(text.to_string()));
                }
            }
        }
        Some("transcript.text.done") => {
            let text = event
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| event.pointer("/transcript").and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            if !text.is_empty() {
                let _ = tx.send(SttTranscript::Final(text));
            }
        }
        _ => {}
    }
}

#[cfg(feature = "live")]
async fn http_transcribe_sse(
    pcm: Bytes,
    api_key: &str,
    transcriptions_url: &str,
    model: &str,
    language: Option<&str>,
    tx: mpsc::UnboundedSender<SttTranscript>,
) -> SpeechResult<()> {
    use futures_util::StreamExt;

    let wav = mono16_le_to_wav(pcm.as_ref());
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("stream", "true");
    if let Some(language) = language {
        form = form.text("language", language.to_string());
    }

    let client = reqwest::Client::new();
    let response = client
        .post(transcriptions_url)
        .bearer_auth(api_key)
        .header("Accept", "text/event-stream")
        .multipart(form)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(SpeechError::Vendor {
            vendor: "openai".into(),
            message: format!("transcription SSE HTTP {}: {}", status, body),
        });
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| SpeechError::Vendor {
            vendor: "openai".into(),
            message: err.to_string(),
        })?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n")) {
            let sep_len = if buffer.get(pos..pos + 4) == Some("\r\n\r\n") {
                4
            } else {
                2
            };
            let frame = buffer[..pos].to_string();
            buffer.drain(..pos + sep_len);
            for line in frame.lines() {
                let line = line.trim();
                if !line.starts_with("data:") {
                    continue;
                }
                let payload = line.trim_start_matches("data:").trim();
                if payload.is_empty() || payload == "[DONE]" {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<Value>(payload) {
                    push_file_sse_event(&event, &tx);
                }
            }
        }
    }
    Ok(())
}

#[async_trait]
impl SttProvider for OpenAiStt {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    fn decode_backlog_ms(&self) -> u32 {
        match &self.transport {
            Transport::Http(http) => http.decode_backlog_ms(),
            Transport::Realtime(rt) => rt.decode_backlog_ms(),
        }
    }

    async fn start(&mut self) -> SpeechResult<()> {
        match &mut self.transport {
            Transport::Http(http) => http.start().await,
            Transport::Realtime(rt) => {
                rt.stop().await?;
                #[cfg(feature = "live")]
                {
                    return rt
                        .start_realtime(&self.api_key, &self.model, self.language.as_deref())
                        .await;
                }
                #[cfg(not(feature = "live"))]
                {
                    Err(SpeechError::Vendor {
                        vendor: "openai".into(),
                        message: "live OpenAI STT requires `--features live` on vendor-openai"
                            .into(),
                    })
                }
            }
        }
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        match &mut self.transport {
            Transport::Http(http) => http.stop().await,
            Transport::Realtime(rt) => rt.stop().await,
        }
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        match &mut self.transport {
            Transport::Http(http) => http.push_audio(pcm).await,
            Transport::Realtime(rt) => rt.push_audio(pcm).await,
        }
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        match &mut self.transport {
            Transport::Http(http) => http.poll_transcript().await,
            Transport::Realtime(rt) => rt.poll_transcript().await,
        }
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        match &mut self.transport {
            Transport::Http(http) => {
                http.finalize_utterance(&self.api_key, &self.model, &self.language)
                    .await
            }
            Transport::Realtime(rt) => rt.finalize_utterance().await,
        }
    }
}

#[cfg(feature = "live")]
impl RealtimeBackend {
    async fn start_realtime(
        &mut self,
        api_key: &Option<String>,
        model: &str,
        language: Option<&str>,
    ) -> SpeechResult<()> {
        let api_key = crate::factory::api_key_from(api_key, "OPENAI_API_KEY")?;
        let ws_url = realtime_websocket_url(model);
        let mut request = ws_url
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
        let session = session_update_body(model, language);
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
        let waiting = self.commit_waiting.load(Ordering::Relaxed);
        let mut inner = self.inner.lock().await;
        let Some(rx) = inner.rx.as_mut() else {
            return Ok(None);
        };
        let recv_next = async {
            match rx.try_recv() {
                Ok(Incoming::Update(transcript)) => Ok(Some(transcript)),
                Ok(Incoming::Failed(message)) => Err(SpeechError::Vendor {
                    vendor: "openai".into(),
                    message,
                }),
                Err(mpsc::error::TryRecvError::Empty) if waiting => {
                    match rx.recv().await {
                        Some(Incoming::Update(transcript)) => Ok(Some(transcript)),
                        Some(Incoming::Failed(message)) => Err(SpeechError::Vendor {
                            vendor: "openai".into(),
                            message,
                        }),
                        None => Err(SpeechError::Vendor {
                            vendor: "openai".into(),
                            message: "realtime transcription socket closed".into(),
                        }),
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => Ok(None),
                Err(mpsc::error::TryRecvError::Disconnected) => Err(SpeechError::Vendor {
                    vendor: "openai".into(),
                    message: "realtime transcription socket closed".into(),
                }),
            }
        };
        let transcript = recv_next.await?;
        drop(inner);
        if let Some(transcript) = transcript {
            if matches!(transcript, SttTranscript::Final(_)) {
                self.on_final_delivered();
            }
            Ok(Some(transcript))
        } else {
            Ok(None)
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

#[cfg(not(feature = "live"))]
impl RealtimeBackend {
    async fn stop(&mut self) -> SpeechResult<()> {
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        Ok(None)
    }

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
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
        if let Some(
            "session.updated" | "transcription_session.updated" | "transcription_session.created",
        ) = event.get("type").and_then(Value::as_str)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn oneshot_24k(pcm16: &[u8]) -> Vec<u8> {
        let mut resampler = Pcm16kTo24k::default();
        let mut out = resampler.push_s16le(pcm16);
        out.extend(resampler.finish());
        out
    }

    fn test_stt_config(model: &str, endpoint: Option<String>) -> SttConfig {
        SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::Openai,
            model: Some(model.into()),
            model_path: None,
            language: Some("en".into()),
            api_key: Some("test-key".into()),
            endpoint,
        }
    }

    fn pcm_above_min_batch() -> Bytes {
        Bytes::from(vec![0_u8; STT_MIN_BATCH_BYTES + 64])
    }

    #[test]
    fn catalog_models_use_http_not_realtime() {
        assert!(!model_uses_realtime_transcription("whisper-1"));
        assert!(!model_uses_realtime_transcription("gpt-4o-mini-transcribe"));
        assert!(!model_uses_realtime_transcription("gpt-4o-transcribe"));
        assert!(!model_uses_realtime_transcription("gpt-transcribe"));
        assert!(model_uses_realtime_transcription("gpt-live-transcribe"));
    }

    #[test]
    fn transcriptions_url_from_api_base() {
        assert_eq!(
            transcriptions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            transcriptions_url("http://127.0.0.1:9/v1"),
            "http://127.0.0.1:9/v1/audio/transcriptions"
        );
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
    fn transcription_session_update_openapi_shape() {
        let body = transcription_session_update_body("gpt-4o-mini-transcribe", Some("en"));
        assert_eq!(body["type"], "transcription_session.update");
        assert_eq!(
            body["session"]["input_audio_transcription"]["model"],
            "gpt-4o-mini-transcribe"
        );
    }

    #[test]
    fn session_update_commits_on_our_vad_not_server_vad() {
        let body = session_update_body("gpt-transcribe", Some("en"));
        assert_eq!(body["type"], "session.update");
        assert_eq!(body["session"]["type"], "transcription");
        assert!(body["session"]["audio"]["input"]["turn_detection"].is_null());
        assert_eq!(
            body["session"]["audio"]["input"]["transcription"]["model"],
            "gpt-transcribe"
        );
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
    fn empty_realtime_completion_is_not_a_final() {
        let mut assembler = TranscriptAssembler::default();
        assert!(assembler
            .push(&serde_json::json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "item_id": "item_1",
                "transcript": ""
            }))
            .is_none());
    }

    #[test]
    fn error_event_is_a_failure_message() {
        let message = realtime_error_message(&serde_json::json!({
            "type": "error",
            "error": { "message": "buffer too small" }
        }));
        assert_eq!(message.as_deref(), Some("buffer too small"));
    }

    #[tokio::test]
    async fn http_poll_does_not_upload_while_buffering() {
        let mut stt =
            OpenAiStt::new(&test_stt_config("gpt-4o-mini-transcribe", None)).expect("stt");
        stt.start().await.expect("start");
        stt.push_audio(pcm_above_min_batch()).await.expect("push");
        assert!(stt.poll_transcript().await.expect("poll").is_none());
        stt.stop().await.expect("stop");
    }

    #[tokio::test]
    async fn http_finalize_below_min_batch_does_not_post() {
        let mut stt =
            OpenAiStt::new(&test_stt_config("gpt-4o-mini-transcribe", None)).expect("stt");
        stt.start().await.expect("start");
        stt.push_audio(Bytes::from(vec![0_u8; STT_MIN_BATCH_BYTES - 1]))
            .await
            .expect("push");
        stt.finalize_utterance().await.expect("finalize");
        assert!(stt.poll_transcript().await.expect("poll").is_none());
    }

    #[cfg(feature = "live")]
    mod live_http {
        use super::*;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        async fn spawn_transcription_mock(model: &str) -> (String, Arc<AtomicUsize>) {
            let posts = Arc::new(AtomicUsize::new(0));
            let posts_c = posts.clone();
            let model = model.to_string();
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    posts_c.fetch_add(1, Ordering::SeqCst);
                    let mut buf = vec![0_u8; 65_536];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    assert!(
                        req.contains("/v1/audio/transcriptions"),
                        "unexpected path: {}",
                        req.lines().next().unwrap_or("")
                    );
                    assert!(req.contains(&model), "request should include model field");
                    let body = r#"{"text":"one. two. three."}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown();
                }
            });
            (format!("http://{addr}/v1"), posts)
        }

        #[tokio::test]
        async fn http_finalize_posts_once_and_poll_returns_final() {
            let (api_base, posts) = spawn_transcription_mock("whisper-1").await;
            let mut stt = OpenAiStt::new(&test_stt_config("whisper-1", Some(api_base)))
                .expect("stt");
            stt.start().await.expect("start");
            stt.push_audio(pcm_above_min_batch()).await.expect("push");
            assert!(stt.poll_transcript().await.expect("poll").is_none());
            stt.finalize_utterance().await.expect("finalize");

            let mut transcript = None;
            for _ in 0..50 {
                if let Some(t) = stt.poll_transcript().await.expect("poll") {
                    transcript = Some(t);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            assert_eq!(
                transcript,
                Some(SttTranscript::Final("one. two. three.".into()))
            );
            assert_eq!(posts.load(Ordering::SeqCst), 1);
        }

        #[tokio::test]
        async fn http_finalize_sse_emits_partial_then_final() {
            let posts = Arc::new(AtomicUsize::new(0));
            let posts_c = posts.clone();
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    posts_c.fetch_add(1, Ordering::SeqCst);
                    let mut buf = vec![0_u8; 65_536];
                    let _ = stream.read(&mut buf).await;
                    let body = [
                        "data: {\"type\":\"transcript.text.delta\",\"delta\":\"one.\"}\n\n",
                        "data: {\"type\":\"transcript.text.done\",\"text\":\"one. two. three.\"}\n\n",
                    ]
                    .join("");
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            let api_base = format!("http://{addr}/v1");
            let mut stt =
                OpenAiStt::new(&test_stt_config("gpt-4o-mini-transcribe", Some(api_base)))
                    .expect("stt");
            stt.start().await.expect("start");
            stt.push_audio(pcm_above_min_batch()).await.expect("push");
            stt.finalize_utterance().await.expect("finalize");
            let partial = stt.poll_transcript().await.expect("poll");
            assert_eq!(partial, Some(SttTranscript::Partial("one.".into())));
            let final_t = stt.poll_transcript().await.expect("poll");
            assert_eq!(
                final_t,
                Some(SttTranscript::Final("one. two. three.".into()))
            );
            assert_eq!(posts.load(Ordering::SeqCst), 1);
        }

        #[tokio::test]
        async fn http_empty_transcription_response_is_not_final() {
            let posts = Arc::new(AtomicUsize::new(0));
            let posts_c = posts.clone();
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    posts_c.fetch_add(1, Ordering::SeqCst);
                    let mut buf = vec![0_u8; 65_536];
                    let _ = stream.read(&mut buf).await;
                    let body = r#"{"text":""}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            let api_base = format!("http://{addr}/v1");
            let mut stt =
                OpenAiStt::new(&test_stt_config("whisper-1", Some(api_base))).expect("stt");
            stt.start().await.expect("start");
            stt.push_audio(pcm_above_min_batch()).await.expect("push");
            stt.finalize_utterance().await.expect("finalize");
            for _ in 0..50 {
                if stt.poll_transcript().await.expect("poll").is_none() {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    continue;
                }
                panic!("empty vendor text must not surface as Final");
            }
            assert_eq!(posts.load(Ordering::SeqCst), 1);
        }
    }

    #[cfg(not(feature = "live"))]
    #[tokio::test]
    async fn without_live_feature_http_buffers_but_finalize_needs_live() {
        let mut stt =
            OpenAiStt::new(&test_stt_config("gpt-4o-mini-transcribe", None)).expect("stt");
        stt.start().await.expect("http start does not need live");
        stt.push_audio(pcm_above_min_batch()).await.expect("push");
        let err = stt
            .finalize_utterance()
            .await
            .expect_err("transcribe needs live");
        let SpeechError::Vendor { message, .. } = err else {
            panic!("expected vendor error");
        };
        assert!(message.contains("live"));
    }

    #[cfg(not(feature = "live"))]
    #[tokio::test]
    async fn without_live_feature_realtime_start_errors() {
        let mut stt = OpenAiStt::new(&test_stt_config("gpt-live-transcribe", None)).expect("stt");
        let err = stt.start().await.expect_err("realtime needs live");
        let SpeechError::Vendor { message, .. } = err else {
            panic!("expected vendor error");
        };
        assert!(message.contains("live"));
    }
}
