//! Mistral Voxtral TTS client: `POST /v1/audio/speech` with `stream: true`, answered as
//! server-sent events (`speech.audio.delta`, `speech.audio.done`).
//!
//! Docs: <https://docs.mistral.ai/studio/audio/text-to-speech/generate-speech> and
//! <https://docs.mistral.ai/studio/audio/text-to-speech/stream-speech>.
//!
//! Only documented request fields are sent and only documented event fields are read. There is
//! one transport and no retry or fallback: audio that reached the sink is never sent twice.
//! Request text is never logged and never appears in an error message.

use std::time::Duration;

use base64::Engine;
use bytes::Bytes;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::duration_ms_from_stereo_48k;
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink};
use serde::Serialize;
use tokio::time::Instant;

use crate::error::vendor_error;
use crate::matrix::{tts_url, validate_tts_model, Timeouts, POOL_IDLE_TIMEOUT, TCP_KEEPALIVE};
use crate::wav::WavToStereo48k;

/// Mistral `SpeechRequest` with exactly the fields this vendor uses. Nothing else is sent.
#[derive(Debug, Serialize)]
pub(crate) struct SpeechRequest<'a> {
    model: &'a str,
    input: &'a str,
    voice_id: &'a str,
    response_format: &'static str,
    stream: bool,
}

pub(crate) fn speech_request<'a>(
    text: &'a str,
    voice: &'a str,
    model: &'a str,
) -> SpeechRequest<'a> {
    SpeechRequest {
        model,
        input: text,
        voice_id: voice,
        response_format: "wav",
        stream: true,
    }
}

pub(crate) struct MistralClient {
    api_key: Option<String>,
    base_url: String,
    timeouts: Timeouts,
    /// One HTTP client for the provider's life: connections (and TLS sessions) are reused.
    http: reqwest::Client,
    /// How the last request reached Mistral: `h2` or `http/1.1`.
    last_transport: std::sync::Mutex<Option<String>>,
}

struct Synthesis {
    audio: WavToStereo48k,
    pcm: Vec<u8>,
    cancelled: bool,
    received_delta: bool,
}

impl Synthesis {
    /// Hands converted bytes to the sink exactly once. Returns `false` when the sink was
    /// cancelled and reading must stop.
    fn emit(&mut self, pcm: Bytes, sink: Option<&TtsProgressiveSink>) -> bool {
        if pcm.is_empty() {
            return true;
        }
        self.pcm.extend_from_slice(&pcm);
        if let Some(sink) = sink {
            let duration_ms = duration_ms_from_stereo_48k(pcm.len());
            if !sink.send(TtsAudioChunk { pcm, duration_ms }) {
                self.cancelled = true;
                return false;
            }
        }
        true
    }

    fn cancelled_by(&mut self, sink: Option<&TtsProgressiveSink>) -> bool {
        if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
            self.cancelled = true;
        }
        self.cancelled
    }
}

impl MistralClient {
    pub(crate) fn new(
        api_key: Option<String>,
        base_url: Option<String>,
        timeouts: Timeouts,
    ) -> SpeechResult<Self> {
        let base_url = crate::matrix::resolve_base_url_from_env(base_url.as_deref());
        Ok(Self {
            api_key,
            base_url,
            timeouts,
            http: reqwest::Client::builder()
                .connect_timeout(timeouts.connect)
                .pool_idle_timeout(POOL_IDLE_TIMEOUT)
                .tcp_keepalive(TCP_KEEPALIVE)
                .build()
                .map_err(|err| vendor_error(format!("mistral http client build failed: {err}")))?,
            last_transport: std::sync::Mutex::new(None),
        })
    }

    pub(crate) fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    fn note_transport(&self, value: &str) {
        if let Ok(mut guard) = self.last_transport.lock() {
            *guard = Some(value.to_string());
        }
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("MISTRAL_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing MISTRAL_API_KEY".into()))
    }

    /// Streams one utterance. Returns the whole utterance as 48 kHz stereo s16le (for the
    /// phrase cache); the sink has already received every byte once, as it arrived.
    pub(crate) async fn synthesize(
        &self,
        text: &str,
        voice: &str,
        model: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<u8>> {
        validate_tts_model(model)?;
        let api_key = self.api_key()?;
        let deadline = Instant::now() + self.timeouts.total;
        let mut synth = Synthesis {
            audio: WavToStereo48k::new(),
            pcm: Vec::new(),
            cancelled: false,
            received_delta: false,
        };

        let request = self
            .http
            .post(tts_url(&self.base_url))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&speech_request(text, voice, model));
        let response = self
            .bounded(
                deadline,
                self.timeouts.first_audio,
                "first audio timed out",
                request.send(),
            )
            .await?
            .map_err(|err| {
                vendor_error(format!(
                    "mistral http request failed: {}",
                    err.without_url()
                ))
            })?;

        self.note_transport(if response.version() == reqwest::Version::HTTP_2 {
            "h2"
        } else {
            "http/1.1"
        });
        let status = response.status();
        if !status.is_success() {
            let raw = self
                .bounded(
                    deadline,
                    self.timeouts.chunk_idle,
                    "error body stalled",
                    response.text(),
                )
                .await?
                .unwrap_or_default();
            return Err(vendor_error(format!(
                "mistral http {}: {}",
                status.as_u16(),
                http_error_message(&raw, status)
            )));
        }

        use futures_util::StreamExt;
        let mut sse = SseParser::default();
        let mut body = std::pin::pin!(response.bytes_stream());
        loop {
            if synth.cancelled_by(sink) {
                // Dropping the body closes the connection.
                return Ok(synth.pcm);
            }
            let (bound, message) = if synth.received_delta {
                (self.timeouts.chunk_idle, "stalled")
            } else {
                (self.timeouts.first_audio, "first audio timed out")
            };
            let events = match self.bounded(deadline, bound, message, body.next()).await? {
                None => {
                    // The body ended; a last event without a trailing blank line still counts.
                    let mut tail = Vec::new();
                    tail.extend(sse.finish());
                    if self.handle(tail, &mut synth, sink)? == Flow::Done {
                        return Ok(synth.pcm);
                    }
                    return Err(vendor_error(
                        "mistral tts stream ended before speech.audio.done",
                    ));
                }
                Some(Err(err)) => {
                    return Err(vendor_error(format!(
                        "mistral tts stream failed: {}",
                        err.without_url()
                    )))
                }
                Some(Ok(bytes)) => sse.feed(&bytes),
            };
            if self.handle(events, &mut synth, sink)? == Flow::Done {
                return Ok(synth.pcm);
            }
        }
    }

    /// Applies SSE events in order. `Flow::Done` after `speech.audio.done` or a cancel.
    fn handle(
        &self,
        events: Vec<SseEvent>,
        synth: &mut Synthesis,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Flow> {
        for event in events {
            let value: serde_json::Value = serde_json::from_str(&event.data).map_err(|_| {
                vendor_error("mistral tts stream sent an event whose data is not JSON")
            })?;
            let kind = value
                .get("type")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(event.name);
            match kind.as_deref() {
                Some("speech.audio.delta") => {
                    let encoded = value
                        .get("audio_data")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            vendor_error("mistral tts speech.audio.delta has no audio_data")
                        })?;
                    let wav = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|_| vendor_error("mistral tts audio_data is not valid base64"))?;
                    synth.received_delta = true;
                    if synth.cancelled_by(sink) {
                        return Ok(Flow::Done);
                    }
                    let pcm = synth.audio.push(&wav)?;
                    if !synth.emit(pcm, sink) {
                        return Ok(Flow::Done);
                    }
                }
                Some("speech.audio.done") => {
                    if !synth.audio.header_done() {
                        return Err(vendor_error(
                            "mistral tts stream ended without a complete WAV header",
                        ));
                    }
                    if !synth.cancelled_by(sink) {
                        let tail = synth.audio.finish();
                        synth.emit(tail, sink);
                    }
                    return Ok(Flow::Done);
                }
                // Unknown event types are skipped.
                _ => {}
            }
        }
        Ok(Flow::Continue)
    }

    /// Runs `fut` under the smaller of the stage bound and what is left of the total deadline.
    /// The error names the bound that fired.
    async fn bounded<T>(
        &self,
        deadline: Instant,
        stage: Duration,
        stage_message: &str,
        fut: impl std::future::Future<Output = T>,
    ) -> SpeechResult<T> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let total_fires = remaining <= stage;
        match tokio::time::timeout(remaining.min(stage), fut).await {
            Ok(value) => Ok(value),
            Err(_) if total_fires => Err(vendor_error(format!(
                "mistral http timed out after {} ms",
                self.timeouts.total.as_millis()
            ))),
            Err(_) => Err(vendor_error(format!(
                "mistral http {stage_message} after {} ms",
                stage.as_millis()
            ))),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Done,
}

#[derive(Debug)]
struct SseEvent {
    name: Option<String>,
    data: String,
}

/// Minimal server-sent-events reader: `data:` lines (joined with `\n`), an optional `event:`
/// line, comment lines ignored, a blank line ends an event.
#[derive(Default)]
struct SseParser {
    buf: Vec<u8>,
    name: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            let line = line.strip_suffix('\r').unwrap_or(&line);
            if line.is_empty() {
                events.extend(self.dispatch());
            } else {
                self.line(line);
            }
        }
        events
    }

    /// End of body: a final line without `\n` and an event without a blank line still count.
    fn finish(&mut self) -> Option<SseEvent> {
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned();
            let line = line.strip_suffix('\r').unwrap_or(&line).to_string();
            if !line.is_empty() {
                self.line(&line);
            }
        }
        self.dispatch()
    }

    fn line(&mut self, line: &str) {
        if line.starts_with(':') {
            return;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => self.data.push(value.to_string()),
            "event" => self.name = Some(value.to_string()),
            _ => {}
        }
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let name = self.name.take();
        if self.data.is_empty() {
            return None;
        }
        Some(SseEvent {
            name,
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

/// Builds the error text from a Mistral error body: `type`, `code` and `message`. A 422
/// carries `message.detail[].msg`. Anything else falls back to the status phrase, so an
/// unexpected body is never echoed.
fn http_error_message(raw: &str, status: reqwest::StatusCode) -> String {
    let fallback = || status.canonical_reason().unwrap_or("error").to_string();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return fallback();
    };
    let mut parts = Vec::new();
    if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
        parts.push(format!("type={kind}"));
    }
    match value.get("code") {
        Some(serde_json::Value::String(code)) => parts.push(format!("code={code}")),
        Some(serde_json::Value::Number(code)) => parts.push(format!("code={code}")),
        _ => {}
    }
    let message = match value.get("message") {
        Some(serde_json::Value::String(message)) => Some(message.clone()),
        Some(object) => detail_messages(object),
        None => detail_messages(&value),
    };
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        parts.push(message.chars().take(300).collect());
    }
    if parts.is_empty() {
        fallback()
    } else {
        parts.join(" ")
    }
}

fn detail_messages(value: &serde_json::Value) -> Option<String> {
    let msgs: Vec<&str> = value
        .get("detail")?
        .as_array()?
        .iter()
        .filter_map(|item| item.get("msg").and_then(|m| m.as_str()))
        .collect();
    (!msgs.is_empty()).then(|| msgs.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_exactly_the_documented_fields() {
        let json =
            serde_json::to_value(speech_request("hello", "voice_478", "voxtral-mini-tts-3-0"))
                .unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["input", "model", "response_format", "stream", "voice_id"]
        );
        assert_eq!(json["response_format"], "wav");
        assert_eq!(json["stream"], true);
    }

    #[test]
    fn sse_splits_events_across_feeds_and_crlf() {
        let mut p = SseParser::default();
        let mut events = p.feed(b"event: a\r\ndata: {\"x\"");
        assert!(events.is_empty());
        events.extend(p.feed(b":1}\r\n\r\n: comment\ndata: 2\n"));
        events.extend(p.feed(b"data: 3\n\n"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name.as_deref(), Some("a"));
        assert_eq!(events[0].data, "{\"x\":1}");
        assert_eq!(events[1].data, "2\n3");
        assert!(p.finish().is_none());
    }

    #[test]
    fn sse_finish_flushes_an_unterminated_event() {
        let mut p = SseParser::default();
        assert!(p.feed(b"data: {\"type\":\"x\"}").is_empty());
        assert_eq!(p.finish().unwrap().data, "{\"type\":\"x\"}");
    }
}
