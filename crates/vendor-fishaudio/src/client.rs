//! Fish Audio TTS clients: chunked HTTP (`POST /v1/tts`, the default transport) and WebSocket
//! live (`/v1/tts/live`, MessagePack, the fallback).
//!
//! WebSocket: <https://docs.fish.audio/api-reference/endpoint/websocket/tts-live>
//! HTTP: <https://docs.fish.audio/api-reference/endpoint/openapi-v1/text-to-speech>
//!
//! Only documented request fields are sent. Request text and audio are never logged and never
//! appear in error messages.
#![cfg_attr(not(feature = "live"), allow(dead_code))]

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::duration_ms_from_stereo_48k;
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink};
use serde::Serialize;

use crate::matrix::{LATENCY, PCM_SAMPLE_RATE};
use crate::pcm::Pcm24kAssembler;

#[cfg(feature = "live")]
use crate::matrix::{
    http_tts_url, validate_model_id, ws_live_url, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT,
    FIRST_AUDIO_TIMEOUT, HTTP_TOTAL_TIMEOUT, POOL_IDLE_TIMEOUT, TCP_KEEPALIVE,
};

const VENDOR: &str = "fishaudio";

pub(crate) fn vendor_error(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: VENDOR.into(),
        message: message.into(),
    }
}

/// Fish `TTSRequest` with exactly the fields this vendor uses. Nothing else is ever sent.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TtsRequest<'a> {
    text: &'a str,
    format: &'static str,
    sample_rate: u32,
    latency: &'static str,
    normalize: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reference_id: Option<&'a str>,
}

pub(crate) fn tts_request<'a>(text: &'a str, reference_id: Option<&'a str>) -> TtsRequest<'a> {
    TtsRequest {
        text,
        format: "pcm",
        sample_rate: PCM_SAMPLE_RATE,
        latency: LATENCY,
        normalize: true,
        reference_id,
    }
}

/// Client-to-server WebSocket events (MessagePack maps).
#[derive(Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum ClientEvent<'a> {
    Start { request: TtsRequest<'a> },
    Text { text: &'a str },
    Flush,
    Stop,
}

/// Server-to-client WebSocket events. Unknown events are ignored.
#[cfg(feature = "live")]
#[derive(serde::Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum ServerEvent {
    Audio {
        #[serde(with = "serde_bytes")]
        audio: Vec<u8>,
    },
    Finish {
        reason: String,
    },
    #[serde(other)]
    Other,
}

/// A finished (or cancelled) synthesis. `mono_48k_stereo` holds 48 kHz stereo s16le (the name
/// follows the plan; the bytes are the whole utterance after upsampling).
#[derive(Debug)]
pub(crate) struct StreamOutcome {
    pub mono_48k_stereo: Vec<u8>,
    /// Read by the unit tests; the provider relies on the sink's own cancel flag.
    #[allow(dead_code)]
    pub emitted_any: bool,
    #[allow(dead_code)]
    pub cancelled: bool,
}

/// A failed synthesis. `emitted_any` says whether audio already reached the sink, which
/// decides whether a retry on the other transport would duplicate audio.
#[derive(Debug)]
pub(crate) struct StreamFailure {
    pub error: SpeechError,
    pub emitted_any: bool,
    /// HTTP status of a non-2xx answer; `None` for transport errors, timeouts, WebSocket
    /// failures and local errors.
    pub http_status: Option<u16>,
}

impl StreamFailure {
    /// The other transport may be tried for this failure. A client error about the request, the
    /// key or the credit (any 4xx except 429) fails the same way on the WebSocket, so it is
    /// returned as is. Connect and transport errors, timeouts, 429 and 5xx may fall back.
    #[cfg(feature = "live")]
    pub(crate) fn allows_ws_fallback(&self) -> bool {
        match self.http_status {
            None | Some(429) => true,
            Some(status) => status >= 500,
        }
    }
}

struct StreamState {
    assembler: Pcm24kAssembler,
    pcm: Vec<u8>,
    emitted_any: bool,
    cancelled: bool,
    received_audio: bool,
    /// Set by `http_run` when the server answers with a non-2xx status.
    http_status: Option<u16>,
}

impl StreamState {
    fn new() -> SpeechResult<Self> {
        Ok(Self {
            assembler: Pcm24kAssembler::new()?,
            pcm: Vec::new(),
            emitted_any: false,
            cancelled: false,
            received_audio: false,
            http_status: None,
        })
    }

    /// Converts one network chunk and hands it to the sink exactly once.
    /// Returns `false` when the sink was cancelled and reading must stop.
    fn push(&mut self, bytes: &[u8], sink: Option<&TtsProgressiveSink>) -> bool {
        if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
            self.cancelled = true;
            return false;
        }
        if !bytes.is_empty() {
            self.received_audio = true;
        }
        let pcm = self.assembler.push(bytes);
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
            self.emitted_any = true;
        }
        true
    }

    fn cancelled_by(&mut self, sink: Option<&TtsProgressiveSink>) -> bool {
        if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
            self.cancelled = true;
        }
        self.cancelled
    }

    /// Flushes the interpolator's tail once, at the end of the stream, to the sink and the
    /// collected bytes. Skipped when the sink was cancelled.
    fn finish(&mut self, sink: Option<&TtsProgressiveSink>) {
        if self.cancelled_by(sink) {
            return;
        }
        let pcm = self.assembler.finish();
        if pcm.is_empty() {
            return;
        }
        self.pcm.extend_from_slice(&pcm);
        if let Some(sink) = sink {
            let duration_ms = duration_ms_from_stereo_48k(pcm.len());
            if sink.send(TtsAudioChunk { pcm, duration_ms }) {
                self.emitted_any = true;
            } else {
                self.cancelled = true;
            }
        }
    }

    fn into_outcome(self) -> StreamOutcome {
        StreamOutcome {
            mono_48k_stereo: self.pcm,
            emitted_any: self.emitted_any,
            cancelled: self.cancelled,
        }
    }
}

pub(crate) struct FishAudioClient {
    api_key: Option<String>,
    base_url: String,
    /// One HTTP client for the provider's life: connections (and TLS sessions) are reused. HTTP
    /// version: ALPN default (h2 when the server offers it, else HTTP/1.1).
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// How the last request reached Fish: `ws`, `h2` or `http/1.1`.
    last_transport: std::sync::Mutex<Option<String>>,
}

impl FishAudioClient {
    pub(crate) fn new(api_key: Option<String>, base_url: Option<String>) -> SpeechResult<Self> {
        let base_url = base_url
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| crate::matrix::DEFAULT_BASE_URL.to_string());
        Ok(Self {
            api_key,
            base_url,
            #[cfg(feature = "live")]
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .pool_idle_timeout(POOL_IDLE_TIMEOUT)
                .tcp_keepalive(TCP_KEEPALIVE)
                .build()
                .map_err(|err| {
                    vendor_error(format!("fishaudio http client build failed: {err}"))
                })?,
            last_transport: std::sync::Mutex::new(None),
        })
    }

    /// How the last request reached Fish: `ws`, `h2` or `http/1.1`.
    pub(crate) fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    #[cfg(feature = "live")]
    fn note_transport(&self, value: &str) {
        if let Ok(mut guard) = self.last_transport.lock() {
            *guard = Some(value.to_string());
        }
    }

    fn api_key(&self) -> SpeechResult<String> {
        self.api_key
            .clone()
            .filter(|key| !key.is_empty())
            .or_else(|| std::env::var("FISH_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing FISH_API_KEY".into()))
    }

    /// Streams over `wss://…/v1/tts/live`.
    pub(crate) async fn synthesize_ws(
        &self,
        text: &str,
        reference_id: Option<&str>,
        model: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> Result<StreamOutcome, StreamFailure> {
        #[cfg(feature = "live")]
        {
            let mut state = StreamState::new().map_err(|error| StreamFailure {
                error,
                emitted_any: false,
                http_status: None,
            })?;
            match self
                .ws_run(text, reference_id, model, sink, &mut state)
                .await
            {
                Ok(()) => {
                    state.finish(sink);
                    Ok(state.into_outcome())
                }
                Err(error) => Err(StreamFailure {
                    error,
                    emitted_any: state.emitted_any,
                    http_status: state.http_status,
                }),
            }
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = (text, reference_id, model, sink, &self.base_url);
            Err(requires_live())
        }
    }

    /// Streams over chunked `POST {base}/v1/tts`.
    pub(crate) async fn synthesize_http(
        &self,
        text: &str,
        reference_id: Option<&str>,
        model: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> Result<StreamOutcome, StreamFailure> {
        #[cfg(feature = "live")]
        {
            let mut state = StreamState::new().map_err(|error| StreamFailure {
                error,
                emitted_any: false,
                http_status: None,
            })?;
            match self
                .http_run(text, reference_id, model, sink, &mut state)
                .await
            {
                Ok(()) => {
                    state.finish(sink);
                    Ok(state.into_outcome())
                }
                Err(error) => Err(StreamFailure {
                    error,
                    emitted_any: state.emitted_any,
                    http_status: state.http_status,
                }),
            }
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = (text, reference_id, model, sink, &self.base_url);
            if let Err(error) = self.api_key() {
                return Err(StreamFailure {
                    error,
                    emitted_any: false,
                    http_status: None,
                });
            }
            Err(requires_live())
        }
    }
}

#[cfg(not(feature = "live"))]
fn requires_live() -> StreamFailure {
    StreamFailure {
        error: vendor_error("live Fish Audio TTS requires `--features live` on vendor-fishaudio"),
        emitted_any: false,
        http_status: None,
    }
}

#[cfg(feature = "live")]
type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

#[cfg(feature = "live")]
fn header_value(value: &str) -> SpeechResult<tokio_tungstenite::tungstenite::http::HeaderValue> {
    tokio_tungstenite::tungstenite::http::HeaderValue::from_str(value)
        .map_err(|_| SpeechError::Config("fishaudio header value has invalid characters".into()))
}

#[cfg(feature = "live")]
async fn ws_send(ws: &mut Ws, event: &ClientEvent<'_>) -> SpeechResult<()> {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let bytes = rmp_serde::to_vec_named(event)
        .map_err(|err| vendor_error(format!("fishaudio ws encode failed: {err}")))?;
    match tokio::time::timeout(CONNECT_TIMEOUT, ws.send(Message::Binary(bytes.into()))).await {
        Err(_) => Err(vendor_error(format!(
            "fishaudio ws send timed out after {} ms",
            CONNECT_TIMEOUT.as_millis()
        ))),
        Ok(Err(err)) => Err(vendor_error(format!("fishaudio ws send failed: {err}"))),
        Ok(Ok(())) => Ok(()),
    }
}

/// Sends a close frame without waiting for the server's answer; dropping `ws` ends the socket.
#[cfg(feature = "live")]
async fn ws_close(mut ws: Ws) {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let _ = tokio::time::timeout(CONNECT_TIMEOUT, ws.send(Message::Close(None))).await;
}

#[cfg(feature = "live")]
impl FishAudioClient {
    async fn ws_run(
        &self,
        text: &str,
        reference_id: Option<&str>,
        model: &str,
        sink: Option<&TtsProgressiveSink>,
        state: &mut StreamState,
    ) -> SpeechResult<()> {
        use futures_util::StreamExt;
        use tokio_tungstenite::{
            connect_async,
            tungstenite::{client::IntoClientRequest, Message},
        };

        validate_model_id(model).map_err(SpeechError::Config)?;
        let api_key = self.api_key()?;

        let mut request = ws_live_url(&self.base_url)
            .into_client_request()
            .map_err(|err| vendor_error(format!("fishaudio ws request invalid: {err}")))?;
        request
            .headers_mut()
            .insert("Authorization", header_value(&format!("Bearer {api_key}"))?);
        request.headers_mut().insert("model", header_value(model)?);

        let mut ws = match tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request)).await {
            Err(_) => {
                return Err(vendor_error(format!(
                    "fishaudio ws connect timed out after {} ms",
                    CONNECT_TIMEOUT.as_millis()
                )))
            }
            Ok(Err(err)) => {
                return Err(vendor_error(format!("fishaudio ws connect failed: {err}")))
            }
            Ok(Ok((ws, _))) => ws,
        };
        self.note_transport("ws");

        ws_send(
            &mut ws,
            &ClientEvent::Start {
                request: tts_request("", reference_id),
            },
        )
        .await?;
        ws_send(&mut ws, &ClientEvent::Text { text }).await?;
        ws_send(&mut ws, &ClientEvent::Flush).await?;
        ws_send(&mut ws, &ClientEvent::Stop).await?;

        loop {
            if state.cancelled_by(sink) {
                ws_close(ws).await;
                return Ok(());
            }
            let (bound, bound_message) = if state.received_audio {
                (CHUNK_IDLE_TIMEOUT, "stalled")
            } else {
                (FIRST_AUDIO_TIMEOUT, "first audio timed out")
            };
            let message = match tokio::time::timeout(bound, ws.next()).await {
                Err(_) => {
                    return Err(vendor_error(format!(
                        "fishaudio ws {bound_message} after {} ms",
                        bound.as_millis()
                    )))
                }
                Ok(None) => return Err(vendor_error("fishaudio ws closed before finish")),
                Ok(Some(Err(err))) => {
                    return Err(vendor_error(format!("fishaudio ws read failed: {err}")))
                }
                Ok(Some(Ok(message))) => message,
            };
            match message {
                Message::Binary(bytes) => {
                    let event: ServerEvent = rmp_serde::from_slice(&bytes).map_err(|err| {
                        vendor_error(format!("fishaudio ws frame decode failed: {err}"))
                    })?;
                    match event {
                        ServerEvent::Audio { audio } => {
                            if !state.push(&audio, sink) {
                                ws_close(ws).await;
                                return Ok(());
                            }
                        }
                        ServerEvent::Finish { reason } if reason == "stop" => return Ok(()),
                        ServerEvent::Finish { reason } => {
                            return Err(vendor_error(format!(
                                "fishaudio ws finished with reason {reason}"
                            )))
                        }
                        ServerEvent::Other => {}
                    }
                }
                Message::Close(_) => return Err(vendor_error("fishaudio ws closed before finish")),
                _ => {}
            }
        }
    }

    async fn http_run(
        &self,
        text: &str,
        reference_id: Option<&str>,
        model: &str,
        sink: Option<&TtsProgressiveSink>,
        state: &mut StreamState,
    ) -> SpeechResult<()> {
        use futures_util::StreamExt;
        use tokio::time::Instant;

        validate_model_id(model).map_err(SpeechError::Config)?;
        let api_key = self.api_key()?;
        let deadline = Instant::now() + HTTP_TOTAL_TIMEOUT;

        let body = tts_request(text, reference_id);
        let request = self
            .http
            .post(http_tts_url(&self.base_url))
            .header("Authorization", format!("Bearer {api_key}"))
            .header("model", model)
            .json(&body);
        let response = bounded(
            deadline,
            FIRST_AUDIO_TIMEOUT,
            "first audio timed out",
            request.send(),
        )
        .await?
        .map_err(|err| vendor_error(format!("fishaudio http request failed: {err}")))?;

        self.note_transport(if response.version() == reqwest::Version::HTTP_2 {
            "h2"
        } else {
            "http/1.1"
        });
        let status = response.status();
        if !status.is_success() {
            state.http_status = Some(status.as_u16());
            let raw = bounded(deadline, CHUNK_IDLE_TIMEOUT, "stalled", response.text())
                .await?
                .unwrap_or_default();
            return Err(vendor_error(format!(
                "fishaudio http {}: {}",
                status.as_u16(),
                http_error_message(&raw, status)
            )));
        }

        let mut stream = std::pin::pin!(response.bytes_stream());
        loop {
            if state.cancelled_by(sink) {
                // Dropping the body closes the connection.
                return Ok(());
            }
            let (bound, bound_message) = if state.received_audio {
                (CHUNK_IDLE_TIMEOUT, "stalled")
            } else {
                (FIRST_AUDIO_TIMEOUT, "first audio timed out")
            };
            match bounded(deadline, bound, bound_message, stream.next()).await? {
                None => return Ok(()),
                Some(Err(err)) => {
                    return Err(vendor_error(format!("fishaudio http stream failed: {err}")))
                }
                Some(Ok(bytes)) => {
                    if !state.push(&bytes, sink) {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Runs `fut` under the smaller of the stage bound and what is left of the total deadline.
/// The error names the bound that fired.
#[cfg(feature = "live")]
async fn bounded<T>(
    deadline: tokio::time::Instant,
    stage: std::time::Duration,
    stage_message: &str,
    fut: impl std::future::Future<Output = T>,
) -> SpeechResult<T> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let total_fires = remaining <= stage;
    match tokio::time::timeout(remaining.min(stage), fut).await {
        Ok(value) => Ok(value),
        Err(_) if total_fires => Err(vendor_error(format!(
            "fishaudio http timed out after {} ms",
            HTTP_TOTAL_TIMEOUT.as_millis()
        ))),
        Err(_) => Err(vendor_error(format!(
            "fishaudio http {stage_message} after {} ms",
            stage.as_millis()
        ))),
    }
}

/// Fish error bodies are `{status, message, reason}`. Anything else falls back to the status
/// phrase, so an unexpected body is never echoed.
#[cfg(feature = "live")]
fn http_error_message(raw: &str, status: reqwest::StatusCode) -> String {
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        message: Option<String>,
    }
    serde_json::from_str::<ErrorBody>(raw)
        .ok()
        .and_then(|body| body.message)
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(200).collect())
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders through MessagePack, the WS wire format, into a string-keyed map.
    #[cfg(feature = "live")]
    fn wire_fields(request: &TtsRequest<'_>) -> serde_json::Map<String, serde_json::Value> {
        let bytes = rmp_serde::to_vec_named(request).unwrap();
        rmp_serde::from_slice(&bytes).unwrap()
    }

    #[cfg(feature = "live")]
    #[test]
    fn request_carries_exactly_the_documented_fields() {
        let json = wire_fields(&tts_request("hello", Some("voice-1")));
        let mut keys: Vec<_> = json.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "format",
                "latency",
                "normalize",
                "reference_id",
                "sample_rate",
                "text"
            ]
        );
        assert_eq!(json["format"], "pcm");
        assert_eq!(json["sample_rate"], 24000);
        assert_eq!(json["latency"], "balanced");
        assert_eq!(json["normalize"], true);
    }

    #[cfg(feature = "live")]
    #[test]
    fn reference_id_is_omitted_when_unset() {
        let json = wire_fields(&tts_request("hello", None));
        assert!(!json.contains_key("reference_id"));
    }

    #[test]
    fn stream_state_sends_each_chunk_once_and_stops_when_cancelled() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink = TtsProgressiveSink {
            tx,
            cancel: cancel.clone(),
        };
        let mut state = StreamState::new().unwrap();
        assert!(state.push(&[0_u8; 480], Some(&sink)));
        assert!(state.push(&[0_u8; 3], Some(&sink)));
        assert!(state.push(&[0_u8; 3], Some(&sink)));
        let first = rx.try_recv().unwrap();
        // The first source sample is held until the second arrives: 239 intervals of 8 bytes.
        assert_eq!(first.pcm.len(), 1912);
        assert_eq!(first.duration_ms, 9);
        assert_eq!(rx.try_recv().unwrap().pcm.len(), 8);
        assert_eq!(rx.try_recv().unwrap().pcm.len(), 16);
        assert!(rx.try_recv().is_err(), "no re-send of the full buffer");
        assert!(state.emitted_any);

        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!state.push(&[0_u8; 480], Some(&sink)));
        // A cancelled stream sends no tail.
        state.finish(Some(&sink));
        assert!(rx.try_recv().is_err());
        let outcome = state.into_outcome();
        assert!(outcome.cancelled);
        assert_eq!(outcome.mono_48k_stereo.len(), 1912 + 8 + 16);
    }

    #[test]
    fn missing_key_is_a_config_error() {
        let client = FishAudioClient::new(None, None).unwrap();
        if std::env::var("FISH_API_KEY").is_err() {
            assert!(matches!(client.api_key(), Err(SpeechError::Config(_))));
        }
    }
}
