//! Cartesia TTS REST + WebSocket clients.
//!
//! REST bytes: <https://docs.cartesia.ai/api-reference/tts/bytes>
//! WebSocket: <https://docs.cartesia.ai/api-reference/tts/websocket>
//!
//! Streaming basis (TTS endpoint comparison): "All three endpoints stream audio as it is
//! produced." (bytes delivers one HTTP response body, SSE and WebSocket chunk it into events or
//! messages). On `/tts/bytes` the body is therefore read as it arrives. WebSocket: "WebSocket
//! amortizes that cost when you keep the socket open"; the server answers with "a stream of
//! messages with the same `context_id` as your request"; cancel is `{"context_id": "...",
//! "cancel": true}`. No idle timeout or maximum connection age is documented for the TTS
//! WebSocket, so a kept socket is reused only while it was used within [`WS_IDLE_REUSE`].

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::TtsProgressiveSink;

use crate::matrix::{validate_model_id, DEFAULT_API_VERSION, DEFAULT_BASE_URL};
#[cfg(feature = "live")]
use crate::matrix::{
    websocket_url, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, FIRST_AUDIO_TIMEOUT, HTTP_TOTAL_TIMEOUT,
    POOL_IDLE_TIMEOUT, TCP_KEEPALIVE, WS_IDLE_REUSE,
};

pub struct CartesiaClient {
    api_key: Option<String>,
    base_url: String,
    api_version: String,
    /// One REST client for the provider's life: connections (and TLS sessions) are reused. HTTP
    /// version: ALPN default (h2 when the server offers it, else HTTP/1.1).
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// How the last request reached Cartesia: `ws`, `h2` or `http/1.1`.
    last_transport: std::sync::Mutex<Option<String>>,
    /// The TTS socket kept open across utterances (see `synthesize_websocket`).
    #[cfg(feature = "live")]
    ws: tokio::sync::Mutex<Option<WsConn>>,
}

#[cfg(feature = "live")]
fn vendor_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "cartesia".into(),
        message: message.into(),
    }
}

#[cfg(feature = "live")]
fn build_http_client() -> SpeechResult<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(HTTP_TOTAL_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE)
        .build()
        .map_err(|err| SpeechError::Vendor {
            vendor: "cartesia".into(),
            message: format!("cartesia http client build failed: {err}"),
        })
}

/// Result of a WebSocket synthesis that reached `done` (or was cancelled by the sink).
#[cfg(feature = "live")]
pub struct WsOutcome {
    /// All mono s16le 48 kHz PCM received.
    pub mono: Vec<u8>,
    /// At least one chunk was accepted by the sink.
    pub emitted_any: bool,
    /// Stopped early because the sink was cancelled.
    pub cancelled: bool,
}

/// A WebSocket synthesis that failed.
#[cfg(feature = "live")]
pub struct WsFailure {
    pub error: SpeechError,
    /// Audio already reached the sink before the failure; the caller must not fall back.
    pub emitted_any: bool,
}

impl CartesiaClient {
    #[cfg(test)]
    pub fn new(api_key: Option<String>) -> Self {
        Self::with_endpoint(api_key, None).expect("client")
    }

    /// `endpoint` replaces the default `https://api.cartesia.ai` (REST base; WS is derived).
    pub fn with_endpoint(api_key: Option<String>, endpoint: Option<String>) -> SpeechResult<Self> {
        let base_url = endpoint
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Ok(Self {
            api_key,
            base_url,
            api_version: DEFAULT_API_VERSION.to_string(),
            #[cfg(feature = "live")]
            http: build_http_client()?,
            last_transport: std::sync::Mutex::new(None),
            #[cfg(feature = "live")]
            ws: tokio::sync::Mutex::new(None),
        })
    }

    /// How the last request reached Cartesia: `ws`, `h2` or `http/1.1`.
    pub fn last_transport(&self) -> Option<String> {
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
            .or_else(|| std::env::var("CARTESIA_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing CARTESIA_API_KEY".into()))
    }

    /// `POST /tts/bytes`. The response body is forwarded to `sink` as it arrives, each chunk
    /// once; the return value is the same audio. A body error after audio reached the sink is an
    /// error and nothing is retried (`one-send-path-per-unit.md`). Empty audio is an error.
    pub async fn synthesize_text(
        &self,
        text: &str,
        voice: &str,
        model: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<u8>> {
        validate_model_id(model).map_err(SpeechError::Config)?;
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let body = serde_json::json!({
                "model_id": model,
                "transcript": text,
                "voice": {
                    "mode": "id",
                    "id": voice
                },
                "output_format": {
                    "container": "raw",
                    "encoding": "pcm_s16le",
                    "sample_rate": 48000
                }
            });

            let first_audio_deadline = tokio::time::Instant::now() + FIRST_AUDIO_TIMEOUT;
            let first_audio_timed_out = || {
                vendor_err(format!(
                    "cartesia http first audio timed out after {} ms",
                    FIRST_AUDIO_TIMEOUT.as_millis()
                ))
            };
            let mut response = tokio::time::timeout_at(
                first_audio_deadline,
                self.http
                    .post(format!("{}/tts/bytes", self.base_url))
                    .header("X-API-Key", api_key)
                    .header("Cartesia-Version", &self.api_version)
                    .json(&body)
                    .send(),
            )
            .await
            .map_err(|_| first_audio_timed_out())?
            .map_err(|err| vendor_err(err.to_string()))?;

            self.note_transport(if response.version() == reqwest::Version::HTTP_2 {
                "h2"
            } else {
                "http/1.1"
            });
            let status = response.status();
            if !status.is_success() {
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(vendor_err(message));
            }

            let mut mono = Vec::new();
            // An odd trailing byte of one body chunk is the first half of the next sample.
            let mut carry: Option<u8> = None;
            loop {
                let next = if mono.is_empty() {
                    tokio::time::timeout_at(first_audio_deadline, response.chunk())
                        .await
                        .map_err(|_| first_audio_timed_out())?
                } else {
                    tokio::time::timeout(CHUNK_IDLE_TIMEOUT, response.chunk())
                        .await
                        .map_err(|_| {
                            vendor_err(format!(
                                "cartesia http stalled after {} ms",
                                CHUNK_IDLE_TIMEOUT.as_millis()
                            ))
                        })?
                };
                if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
                    break;
                }
                // A body that ends early or errors (reset, bad chunk framing, short
                // Content-Length) is an error, never partial success.
                let Some(chunk) = next.map_err(|err| vendor_err(err.to_string()))? else {
                    break;
                };
                let mut bytes = chunk.to_vec();
                if let Some(first) = carry.take() {
                    bytes.insert(0, first);
                }
                if bytes.len() % 2 == 1 {
                    carry = bytes.pop();
                }
                if bytes.is_empty() {
                    continue;
                }
                mono.extend_from_slice(&bytes);
                if let Some(s) = sink {
                    let _ = s.send(pcm_chunk(&bytes));
                }
            }
            if mono.is_empty() {
                return Err(vendor_err("Cartesia bytes returned no audio"));
            }
            Ok(mono)
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, voice, model, sink, &self.base_url, &self.api_version);
            let _ = self.api_key()?;
            Err(SpeechError::Vendor {
                vendor: "cartesia".into(),
                message: "live Cartesia TTS requires `--features live` on vendor-cartesia".into(),
            })
        }
    }

    #[cfg(feature = "live")]
    async fn open_socket(&self, api_key: &str) -> SpeechResult<WsConn> {
        let url = websocket_url(&self.base_url, &self.api_version);
        let ws = crate::ws_connect::connect(url, api_key, &self.api_version).await?;
        self.note_transport("ws");
        Ok(WsConn {
            ws,
            last_used: tokio::time::Instant::now(),
        })
    }

    /// WebSocket synthesis. One socket is kept per client and reused for the next utterance, each
    /// utterance on a fresh `context_id`. The socket opens on the first utterance, is reused
    /// while it was used within [`WS_IDLE_REUSE`] and still open, and is never reused after an
    /// error or a cancel (a cancel sends the documented `cancel` message, then closes). A
    /// failure on a reused socket before any audio arrived may retry once on a fresh socket.
    #[cfg(feature = "live")]
    pub async fn synthesize_websocket(
        &self,
        text: &str,
        voice: &str,
        model: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> Result<WsOutcome, WsFailure> {
        // Failures before the first chunk cannot have reached the sink.
        let early = |error: SpeechError| WsFailure {
            error,
            emitted_any: false,
        };

        validate_model_id(model)
            .map_err(SpeechError::Config)
            .map_err(early)?;
        let api_key = self.api_key().map_err(early)?;

        // Held for the whole utterance: one provider speaks one utterance at a time.
        let mut slot = self.ws.lock().await;
        let mut reused = false;
        let mut conn = match slot.take() {
            Some(conn) if conn.last_used.elapsed() < WS_IDLE_REUSE => {
                reused = true;
                conn
            }
            Some(old) => {
                close_quietly(old).await;
                self.open_socket(&api_key).await.map_err(early)?
            }
            None => self.open_socket(&api_key).await.map_err(early)?,
        };

        let mut result = run_utterance(&mut conn.ws, text, voice, model, sink.as_ref()).await;
        if let Err(fail) = &result {
            // The server closed the idle socket: nothing reached the sink and no audio arrived,
            // so one fresh socket may retry (`one-send-path-per-unit`).
            if reused && fail.dead && !fail.received && !fail.emitted_any {
                drop(conn);
                conn = self.open_socket(&api_key).await.map_err(early)?;
                result = run_utterance(&mut conn.ws, text, voice, model, sink.as_ref()).await;
            }
        }
        match result {
            Ok(done) if done.end == UttEnd::Done => {
                conn.last_used = tokio::time::Instant::now();
                *slot = Some(conn);
                Ok(WsOutcome {
                    mono: done.mono,
                    emitted_any: done.emitted_any,
                    cancelled: false,
                })
            }
            Ok(done) => {
                close_quietly(conn).await;
                Ok(WsOutcome {
                    mono: done.mono,
                    emitted_any: done.emitted_any,
                    cancelled: true,
                })
            }
            Err(fail) => {
                drop(conn);
                Err(WsFailure {
                    error: fail.error,
                    emitted_any: fail.emitted_any,
                })
            }
        }
    }
}

/// An open TTS socket and when it last finished an utterance.
#[cfg(feature = "live")]
struct WsConn {
    ws: crate::ws_connect::Socket,
    last_used: tokio::time::Instant,
}

#[cfg(feature = "live")]
async fn close_quietly(mut conn: WsConn) {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let _ = tokio::time::timeout(CONNECT_TIMEOUT, conn.ws.send(Message::Close(None))).await;
}

#[cfg(feature = "live")]
#[derive(PartialEq, Eq)]
enum UttEnd {
    /// `done` arrived; the socket is idle and may serve the next utterance.
    Done,
    /// The sink was cancelled; leftover audio may still be in flight, so the socket is closed.
    Cancelled,
}

#[cfg(feature = "live")]
struct UttOk {
    mono: Vec<u8>,
    emitted_any: bool,
    end: UttEnd,
}

#[cfg(feature = "live")]
struct UttFail {
    error: SpeechError,
    /// A chunk was accepted by the sink.
    emitted_any: bool,
    /// Non-empty audio arrived (whether or not a sink took it).
    received: bool,
    /// The socket was closed or broken (send or read failed), as opposed to a timeout or a
    /// vendor `error` message.
    dead: bool,
}

#[cfg(feature = "live")]
fn pcm_chunk(mono: &[u8]) -> node_webrtc_rust_speech::pipeline::TtsAudioChunk {
    use node_webrtc_rust_speech::pcm::{
        duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
    };
    node_webrtc_rust_speech::pipeline::TtsAudioChunk {
        pcm: mono_s16le_to_stereo(mono),
        duration_ms: duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE),
    }
}

/// One generation request on an open socket (fresh `context_id`, `continue: false`), up to its
/// `done`. Messages of any other `context_id` are ignored.
#[cfg(feature = "live")]
async fn run_utterance(
    ws: &mut crate::ws_connect::Socket,
    text: &str,
    voice: &str,
    model: &str,
    sink: Option<&TtsProgressiveSink>,
) -> Result<UttOk, UttFail> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    static CONTEXT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let context_id = format!(
        "ctx-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        CONTEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut emitted_any = false;
    let mut received = false;
    let fail = |message: String, emitted_any: bool, received: bool, dead: bool| UttFail {
        error: vendor_err(message),
        emitted_any,
        received,
        dead,
    };

    let gen = serde_json::json!({
        "model_id": model,
        "transcript": text,
        "voice": { "id": voice },
        "output_format": {
            "container": "raw",
            "encoding": "pcm_s16le",
            "sample_rate": 48000
        },
        "context_id": context_id,
        "continue": false
    });
    match tokio::time::timeout(
        CONNECT_TIMEOUT,
        ws.send(Message::Text(gen.to_string().into())),
    )
    .await
    {
        Err(_) => {
            return Err(fail(
                format!(
                    "cartesia tts websocket send timed out after {} s",
                    CONNECT_TIMEOUT.as_secs()
                ),
                false,
                false,
                false,
            ))
        }
        Ok(Err(err)) => return Err(fail(err.to_string(), false, false, true)),
        Ok(Ok(())) => {}
    }

    let mut mono = Vec::new();
    loop {
        let (bound, bound_message) = if received {
            (CHUNK_IDLE_TIMEOUT, "stalled")
        } else {
            (FIRST_AUDIO_TIMEOUT, "first audio timed out")
        };
        let next = match tokio::time::timeout(bound, ws.next()).await {
            Ok(next) => next,
            Err(_) => {
                return Err(fail(
                    format!("cartesia ws {bound_message} after {} ms", bound.as_millis()),
                    emitted_any,
                    received,
                    false,
                ))
            }
        };
        if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
            // Documented cancel of this context; the socket is closed afterwards in any case.
            let cancel = serde_json::json!({ "context_id": context_id, "cancel": true });
            let _ = tokio::time::timeout(
                CONNECT_TIMEOUT,
                ws.send(Message::Text(cancel.to_string().into())),
            )
            .await;
            return Ok(UttOk {
                mono,
                emitted_any,
                end: UttEnd::Cancelled,
            });
        }
        let raw = match next {
            None | Some(Ok(Message::Close(_))) => {
                return Err(fail(
                    "cartesia ws closed before done".into(),
                    emitted_any,
                    received,
                    true,
                ))
            }
            Some(Err(err)) => return Err(fail(err.to_string(), emitted_any, received, true)),
            Some(Ok(Message::Text(raw))) => raw,
            // Ping/Pong are answered by tungstenite; Binary/Frame are not part of the
            // Cartesia TTS WS API.
            Some(Ok(_)) => continue,
        };
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|err| fail(err.to_string(), emitted_any, received, false))?;
        if value
            .get("context_id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id != context_id)
        {
            continue;
        }
        match value.get("type").and_then(|v| v.as_str()) {
            Some("done") if value.get("done").and_then(|v| v.as_bool()) == Some(true) => {
                break;
            }
            Some("error") => {
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.get("title").and_then(|v| v.as_str()))
                    .unwrap_or("cartesia ws error");
                return Err(fail(message.to_string(), emitted_any, received, false));
            }
            Some("chunk") => {
                if let Some(data_b64) = value.get("data").and_then(|v| v.as_str()) {
                    use base64::{engine::general_purpose::STANDARD, Engine as _};
                    let bytes = STANDARD
                        .decode(data_b64)
                        .map_err(|err| fail(err.to_string(), emitted_any, received, false))?;
                    mono.extend_from_slice(&bytes);
                    if !bytes.is_empty() {
                        received = true;
                    }
                    if let Some(s) = sink {
                        if s.send(pcm_chunk(&bytes)) {
                            emitted_any = true;
                        }
                    }
                }
            }
            // timestamps, phoneme_timestamps, flush_done, ...
            _ => {}
        }
    }
    Ok(UttOk {
        mono,
        emitted_any,
        end: UttEnd::Done,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_api_version_matches_matrix() {
        let client = CartesiaClient::new(Some("k".into()));
        assert_eq!(client.api_version, DEFAULT_API_VERSION);
    }
}
