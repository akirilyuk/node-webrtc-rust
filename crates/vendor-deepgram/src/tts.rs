use async_trait::async_trait;
use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::tts_matrix::{
    clear_json, default_speak_model, flush_json, resolve_tts_transport, speak_json,
    tts_progressive_transport, v1_speak_rest_url, v1_speak_websocket_url, v2_speak_rest_url,
    v2_speak_websocket_url, TtsProgressiveTransport, DEFAULT_BASE_URL,
};
#[cfg(feature = "live")]
use crate::tts_matrix::{
    CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, FIRST_AUDIO_TIMEOUT, HTTP_TOTAL_TIMEOUT,
    POOL_IDLE_TIMEOUT, TCP_KEEPALIVE, WS_CONNECT_TIMEOUT, WS_MAX_AGE,
};

pub struct DeepgramTts {
    api_key: Option<String>,
    model: String,
    /// Resolved transport: `Websocket` streams progressively, `Http` posts to the REST endpoint.
    transport: SpeechTransport,
    /// REST base (`TtsConfig.endpoint` or [`DEFAULT_BASE_URL`]); the WS host is derived from it.
    #[cfg_attr(not(feature = "live"), allow(dead_code))]
    base_url: String,
    /// One REST client for the provider's life: connections (and TLS sessions) are reused. HTTP
    /// version: ALPN default (h2 when the server offers it, else HTTP/1.1).
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// How the last request reached Deepgram: `ws`, `h2` or `http/1.1`.
    last_transport: std::sync::Mutex<Option<String>>,
    /// The Aura speak socket kept open across utterances (see `synthesize_websocket`).
    #[cfg(feature = "live")]
    ws: tokio::sync::Mutex<Option<WsConn>>,
}

impl DeepgramTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_speak_model().to_string());
        crate::tts_matrix::validate_speak_model(&model).map_err(SpeechError::Config)?;
        let transport = resolve_tts_transport(config.transport)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok()),
            model,
            transport,
            base_url: config
                .endpoint
                .as_deref()
                .map(str::trim)
                .map(|value| value.trim_end_matches('/'))
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_BASE_URL)
                .to_string(),
            #[cfg(feature = "live")]
            http: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(HTTP_TOTAL_TIMEOUT)
                .pool_idle_timeout(POOL_IDLE_TIMEOUT)
                .tcp_keepalive(TCP_KEEPALIVE)
                .build()
                .map_err(|err| SpeechError::Vendor {
                    vendor: "deepgram".into(),
                    message: format!("deepgram http client build failed: {err}"),
                })?,
            last_transport: std::sync::Mutex::new(None),
            #[cfg(feature = "live")]
            ws: tokio::sync::Mutex::new(None),
        })
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
            .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing DEEPGRAM_API_KEY".into()))
    }
}

#[async_trait]
impl TtsProvider for DeepgramTts {
    fn vendor_name(&self) -> &'static str {
        "deepgram"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_rest(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        if self.transport == SpeechTransport::Http {
            // Explicit `http`: the REST body goes to the sink as it arrives, each chunk once.
            return self.synthesize_rest(text, sink.as_ref()).await;
        }
        match tts_progressive_transport(&self.model).map_err(SpeechError::Config)? {
            TtsProgressiveTransport::WebSocketV1 | TtsProgressiveTransport::WebSocketV2 => {
                #[cfg(feature = "live")]
                {
                    return self.synthesize_websocket(text, sink).await;
                }
                #[cfg(not(feature = "live"))]
                {
                    self.synthesize_rest(text, sink.as_ref()).await
                }
            }
            TtsProgressiveTransport::RestFullBody => {
                self.synthesize_rest(text, sink.as_ref()).await
            }
        }
    }
}

impl DeepgramTts {
    /// `POST /v1/speak` (or `/v2/speak`). The response body is forwarded to `sink` as it arrives,
    /// each chunk once; the return value is the same audio. A body error after audio reached the
    /// sink is an error and nothing is retried (`one-send-path-per-unit.md`).
    async fn synthesize_rest(
        &self,
        text: &str,
        sink: Option<&TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let url = if self.model.starts_with("flux-") {
                v2_speak_rest_url(&self.base_url, &self.model).map_err(SpeechError::Config)?
            } else {
                v1_speak_rest_url(&self.base_url, &self.model).map_err(SpeechError::Config)?
            };
            let body = serde_json::json!({ "text": text });

            let first_audio_deadline = tokio::time::Instant::now() + FIRST_AUDIO_TIMEOUT;
            let first_audio_timed_out = || {
                vendor_error(format!(
                    "deepgram http first audio timed out after {} ms",
                    FIRST_AUDIO_TIMEOUT.as_millis()
                ))
            };
            let mut response = tokio::time::timeout_at(
                first_audio_deadline,
                self.http
                    .post(url)
                    .header("Authorization", format!("Token {api_key}"))
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send(),
            )
            .await
            .map_err(|_| first_audio_timed_out())?
            .map_err(|err| vendor_error(err.to_string()))?;

            self.note_transport(if response.version() == reqwest::Version::HTTP_2 {
                "h2"
            } else {
                "http/1.1"
            });
            if !response.status().is_success() {
                let status = response.status();
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(vendor_error(message));
            }

            let mut mono = Vec::new();
            let mut wav = WavHeaderSkip::default();
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
                            vendor_error(format!(
                                "deepgram http stalled after {} ms",
                                CHUNK_IDLE_TIMEOUT.as_millis()
                            ))
                        })?
                };
                if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
                    break;
                }
                // A body that ends early or errors (reset, bad chunk framing, short
                // Content-Length) is an error, never partial success.
                let Some(chunk) = next.map_err(|err| vendor_error(err.to_string()))? else {
                    break;
                };
                let mut bytes = wav.feed(chunk.to_vec());
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
                    let _ = s.send(pcm_chunk_from_mono(&bytes));
                }
            }
            if mono.is_empty() {
                return Err(vendor_error("Deepgram speak REST returned no audio"));
            }
            Ok(vec![pcm_chunk_from_mono(&mono)])
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, sink, self.api_key()?);
            Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "live Deepgram TTS requires `--features live` on vendor-deepgram".into(),
            })
        }
    }

    #[cfg(feature = "live")]
    async fn open_socket(&self, url: &str, api_key: &str) -> SpeechResult<WsConn> {
        let ws = crate::ws_connect::connect(url.to_string(), api_key, "tts").await?;
        self.note_transport("ws");
        Ok(WsConn {
            ws,
            opened: tokio::time::Instant::now(),
        })
    }

    /// Speak WebSocket. Aura (`/v1/speak`) keeps one socket per provider across utterances: the
    /// docs describe repeated `Speak` + `Flush` on one open connection, each flush answered by a
    /// `Flushed`. The socket is opened on the first utterance, reused while open, replaced when
    /// it is older than [`WS_MAX_AGE`] or the server closed it, and never reused after an error
    /// or a cancel. Flux (`/v2/speak`) ends an utterance with `SpeechMetadata`, which is not
    /// documented as a per-flush marker, so it keeps one socket per utterance.
    #[cfg(feature = "live")]
    async fn synthesize_websocket(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let api_key = self.api_key()?;
        let flux = self.model.starts_with("flux-");
        let url = if flux {
            v2_speak_websocket_url(&self.base_url, &self.model).map_err(SpeechError::Config)?
        } else {
            v1_speak_websocket_url(&self.base_url, &self.model).map_err(SpeechError::Config)?
        };
        let reusable = !flux;

        // Held for the whole utterance: one provider speaks one utterance at a time.
        let mut slot = self.ws.lock().await;
        let mut reused = false;
        let mut conn = match slot.take() {
            Some(conn) if reusable && conn.opened.elapsed() < WS_MAX_AGE => {
                reused = true;
                conn
            }
            Some(old) => {
                close_quietly(old).await;
                self.open_socket(&url, &api_key).await?
            }
            None => self.open_socket(&url, &api_key).await?,
        };

        let mut result = run_utterance(&mut conn.ws, text, sink.as_ref()).await;
        if let Err(fail) = &result {
            // The server closed the idle socket: nothing reached the sink, so one fresh
            // socket may retry. After audio the error is returned (`one-send-path-per-unit`).
            if reused && fail.dead && !fail.audio {
                drop(conn);
                conn = self.open_socket(&url, &api_key).await?;
                result = run_utterance(&mut conn.ws, text, sink.as_ref()).await;
            }
        }
        match result {
            Ok((mono, UttEnd::Done)) if reusable => {
                *slot = Some(conn);
                Ok(vec![pcm_chunk_from_mono(&mono)])
            }
            Ok((mono, _)) => {
                close_quietly(conn).await;
                Ok(vec![pcm_chunk_from_mono(&mono)])
            }
            Err(fail) => {
                drop(conn);
                Err(fail.err)
            }
        }
    }
}

/// An open speak socket and when it was opened.
#[cfg(feature = "live")]
struct WsConn {
    ws: crate::ws_connect::Socket,
    opened: tokio::time::Instant,
}

#[cfg(feature = "live")]
async fn close_quietly(mut conn: WsConn) {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    let _ = tokio::time::timeout(WS_CONNECT_TIMEOUT, conn.ws.send(Message::Close(None))).await;
}

#[cfg(feature = "live")]
enum UttEnd {
    /// `Flushed` arrived; the socket is idle and may serve the next utterance.
    Done,
    /// The sink was cancelled; leftover audio may still be in flight, so the socket is closed.
    Cancelled,
}

#[cfg(feature = "live")]
struct UttFail {
    err: SpeechError,
    /// Audio of this utterance already reached the sink.
    audio: bool,
    /// The socket was closed or broken (send or read failed), as opposed to a timeout or a
    /// vendor `Error` message.
    dead: bool,
}

/// One `Speak` + `Flush` on an open socket, up to its `Flushed`.
#[cfg(feature = "live")]
async fn run_utterance(
    ws: &mut crate::ws_connect::Socket,
    text: &str,
    sink: Option<&TtsProgressiveSink>,
) -> Result<(Vec<u8>, UttEnd), UttFail> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let fail = |message: String, audio: bool, dead: bool| UttFail {
        err: vendor_error(message),
        audio,
        dead,
    };

    for control in [speak_json(text), flush_json().to_string()] {
        match tokio::time::timeout(WS_CONNECT_TIMEOUT, ws.send(Message::Text(control.into()))).await
        {
            Err(_) => {
                return Err(fail(
                    format!(
                        "deepgram tts websocket send timed out after {} s",
                        WS_CONNECT_TIMEOUT.as_secs()
                    ),
                    false,
                    false,
                ))
            }
            Ok(Err(err)) => return Err(fail(err.to_string(), false, true)),
            Ok(Ok(())) => {}
        }
    }

    let mut mono = Vec::new();
    let mut received_audio = false;
    loop {
        let (bound, bound_message) = if received_audio {
            (CHUNK_IDLE_TIMEOUT, "stalled")
        } else {
            (FIRST_AUDIO_TIMEOUT, "first audio timed out")
        };
        let next = tokio::time::timeout(bound, ws.next()).await.map_err(|_| {
            fail(
                format!("deepgram ws {bound_message} after {} ms", bound.as_millis()),
                received_audio,
                false,
            )
        })?;
        if sink.is_some_and(TtsProgressiveSink::is_cancelled) {
            // Documented `Clear` drops what the server still has queued; the socket is not
            // reused after a cancel in any case.
            let _ = tokio::time::timeout(
                WS_CONNECT_TIMEOUT,
                ws.send(Message::Text(clear_json().to_string().into())),
            )
            .await;
            if mono.is_empty() {
                return Err(fail(
                    "deepgram ws cancelled before audio".into(),
                    false,
                    false,
                ));
            }
            return Ok((mono, UttEnd::Cancelled));
        }
        let msg = match next {
            None | Some(Ok(Message::Close(_))) => {
                return Err(fail(
                    "deepgram ws closed before Flushed".into(),
                    received_audio,
                    true,
                ))
            }
            Some(Err(err)) => {
                return Err(fail(
                    format!("deepgram ws read failed: {err}"),
                    received_audio,
                    true,
                ))
            }
            Some(Ok(msg)) => msg,
        };
        let mut turn_done = false;
        match msg {
            Message::Binary(chunk) => {
                if !chunk.is_empty() {
                    received_audio = true;
                }
                mono.extend_from_slice(&chunk);
                if let Some(s) = sink {
                    let _ = s.send(pcm_chunk_from_mono(&chunk));
                }
            }
            Message::Text(raw) => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                    let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if msg_type == "SpeechMetadata"
                        || msg_type == "Flushed"
                        || msg_type == "Cleared"
                    {
                        turn_done = true;
                    }
                    if msg_type == "Error" {
                        let description = value
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Deepgram speak error");
                        return Err(fail(description.to_string(), received_audio, false));
                    }
                }
            }
            _ => {}
        }
        if turn_done {
            break;
        }
    }
    if mono.is_empty() {
        return Err(fail(
            "Deepgram speak websocket returned no audio".into(),
            false,
            false,
        ));
    }
    Ok((mono, UttEnd::Done))
}

/// Drops the container header the REST endpoint may put in front of the PCM. Deepgram: linear16
/// supports containers `wav` (default) and `none`, "When using the wav container, the response
/// includes standard container metadata" (media output settings). The request keeps the query
/// as it was, so a canonical 44-byte RIFF/WAVE header is skipped when the body starts with one.
#[derive(Default)]
struct WavHeaderSkip {
    head: Vec<u8>,
    decided: bool,
}

impl WavHeaderSkip {
    fn feed(&mut self, incoming: Vec<u8>) -> Vec<u8> {
        if self.decided {
            return incoming;
        }
        self.head.extend_from_slice(&incoming);
        if self.head.len() < 12 {
            return Vec::new();
        }
        let wav = &self.head[0..4] == b"RIFF" && &self.head[8..12] == b"WAVE";
        if wav && self.head.len() < 44 {
            return Vec::new();
        }
        self.decided = true;
        let all = std::mem::take(&mut self.head);
        if wav {
            all[44..].to_vec()
        } else {
            all
        }
    }
}

#[cfg(feature = "live")]
fn vendor_error(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "deepgram".into(),
        message: message.into(),
    }
}

fn pcm_chunk_from_mono(mono: &[u8]) -> TtsAudioChunk {
    let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
    let pcm = mono_s16le_to_stereo(mono);
    TtsAudioChunk { pcm, duration_ms }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn rejects_flux_model_on_aura_only_config_mismatch() {
        assert!(matches!(
            DeepgramTts::new(&TtsConfig {
                provider: TtsVendor::Deepgram,
                model: Some("not-a-model".into()),
                model_path: None,
                voice: None,
                api_key: Some("k".into()),
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            }),
            Err(SpeechError::Config(_))
        ));
    }

    #[test]
    fn accepts_aura_and_flux_models() {
        for model in ["aura-asteria-en", "flux-haley-en"] {
            assert!(DeepgramTts::new(&TtsConfig {
                provider: TtsVendor::Deepgram,
                model: Some(model.into()),
                model_path: None,
                voice: None,
                api_key: Some("k".into()),
                endpoint: None,
                sentence_case: Default::default(),
                transport: None,
            })
            .is_ok());
        }
    }
}
