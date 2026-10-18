//! Cartesia TTS REST + WebSocket clients.
//!
//! REST bytes: <https://docs.cartesia.ai/api-reference/tts/bytes>
//! WebSocket: <https://docs.cartesia.ai/api-reference/tts/websocket>

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
#[cfg(feature = "live")]
use node_webrtc_rust_speech::pipeline::TtsProgressiveSink;

use crate::matrix::{validate_model_id, DEFAULT_API_VERSION, DEFAULT_BASE_URL};
#[cfg(feature = "live")]
use crate::matrix::{
    websocket_url, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, FIRST_AUDIO_TIMEOUT, HTTP_TOTAL_TIMEOUT,
    POOL_IDLE_TIMEOUT, TCP_KEEPALIVE,
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

    pub async fn synthesize_text(
        &self,
        text: &str,
        voice: &str,
        model: &str,
    ) -> SpeechResult<Vec<u8>> {
        validate_model_id(model).map_err(SpeechError::Config)?;
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            if voice == "default" {
                return Err(SpeechError::Config(
                    "set tts.voice or CARTESIA_VOICE_ID to a Cartesia voice id".into(),
                ));
            }

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

            let response = self
                .http
                .post(format!("{}/tts/bytes", self.base_url))
                .header("X-API-Key", api_key)
                .header("Cartesia-Version", &self.api_version)
                .json(&body)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "cartesia".into(),
                    message: err.to_string(),
                })?;

            self.note_transport(if response.version() == reqwest::Version::HTTP_2 {
                "h2"
            } else {
                "http/1.1"
            });
            let status = response.status();
            if !status.is_success() {
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "cartesia".into(),
                    message,
                });
            }

            response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|err| SpeechError::Vendor {
                    vendor: "cartesia".into(),
                    message: err.to_string(),
                })
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, voice, model, &self.base_url, &self.api_version);
            let _ = self.api_key()?;
            Err(SpeechError::Vendor {
                vendor: "cartesia".into(),
                message: "live Cartesia TTS requires `--features live` on vendor-cartesia".into(),
            })
        }
    }

    #[cfg(feature = "live")]
    pub async fn synthesize_websocket(
        &self,
        text: &str,
        voice: &str,
        model: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> Result<WsOutcome, WsFailure> {
        use futures_util::{SinkExt, StreamExt};
        use node_webrtc_rust_speech::pcm::{
            duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
        };
        use node_webrtc_rust_speech::pipeline::TtsAudioChunk;
        use tokio_tungstenite::{
            connect_async,
            tungstenite::{client::IntoClientRequest, Message},
        };

        // Failures before the first chunk cannot have reached the sink.
        let early = |error: SpeechError| WsFailure {
            error,
            emitted_any: false,
        };

        validate_model_id(model)
            .map_err(SpeechError::Config)
            .map_err(early)?;
        let api_key = self.api_key().map_err(early)?;
        if voice == "default" {
            return Err(early(SpeechError::Config(
                "set tts.voice or CARTESIA_VOICE_ID to a Cartesia voice id".into(),
            )));
        }

        let context_id = format!(
            "ctx-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let url = websocket_url(&self.base_url, &self.api_version);
        let mut request = url
            .into_client_request()
            .map_err(|err| early(vendor_err(err.to_string())))?;
        request.headers_mut().insert(
            "X-API-Key",
            api_key
                .parse()
                .map_err(|_| early(vendor_err("invalid CARTESIA_API_KEY header value")))?,
        );
        request.headers_mut().insert(
            "Cartesia-Version",
            self.api_version
                .parse()
                .map_err(|_| early(vendor_err("invalid Cartesia-Version header value")))?,
        );

        let connect_timed_out = || {
            early(vendor_err(format!(
                "cartesia ws connect timed out after {} ms",
                CONNECT_TIMEOUT.as_millis()
            )))
        };
        let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| connect_timed_out())?
            .map_err(|err| early(vendor_err(err.to_string())))?;
        self.note_transport("ws");
        let (mut ws_tx, mut ws_rx) = ws.split();

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
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            ws_tx.send(Message::Text(gen.to_string().into())),
        )
        .await
        .map_err(|_| connect_timed_out())?
        .map_err(|err| early(vendor_err(err.to_string())))?;

        let mut mono = Vec::new();
        let mut emitted_any = false;
        let mut received_audio = false;
        loop {
            let (bound, bound_message) = if received_audio {
                (CHUNK_IDLE_TIMEOUT, "stalled")
            } else {
                (FIRST_AUDIO_TIMEOUT, "first audio timed out")
            };
            let next = match tokio::time::timeout(bound, ws_rx.next()).await {
                Ok(next) => next,
                Err(_) => {
                    return Err(WsFailure {
                        error: vendor_err(format!(
                            "cartesia ws {bound_message} after {} ms",
                            bound.as_millis()
                        )),
                        emitted_any,
                    })
                }
            };
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(2), ws_tx.close()).await;
                return Ok(WsOutcome {
                    mono,
                    emitted_any,
                    cancelled: true,
                });
            }
            let raw = match next {
                None | Some(Ok(Message::Close(_))) => {
                    return Err(WsFailure {
                        error: vendor_err("cartesia ws closed before done"),
                        emitted_any,
                    })
                }
                Some(Err(err)) => {
                    return Err(WsFailure {
                        error: vendor_err(err.to_string()),
                        emitted_any,
                    })
                }
                Some(Ok(Message::Text(raw))) => raw,
                // Ping/Pong are answered by tungstenite; Binary/Frame are not part of the
                // Cartesia TTS WS API.
                Some(Ok(_)) => continue,
            };
            let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| WsFailure {
                error: vendor_err(err.to_string()),
                emitted_any,
            })?;
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
                    return Err(WsFailure {
                        error: vendor_err(message),
                        emitted_any,
                    });
                }
                Some("chunk") => {
                    if let Some(data_b64) = value.get("data").and_then(|v| v.as_str()) {
                        use base64::{engine::general_purpose::STANDARD, Engine as _};
                        let bytes = STANDARD.decode(data_b64).map_err(|err| WsFailure {
                            error: vendor_err(err.to_string()),
                            emitted_any,
                        })?;
                        mono.extend_from_slice(&bytes);
                        if !bytes.is_empty() {
                            received_audio = true;
                        }
                        if let Some(s) = sink.as_ref() {
                            let duration_ms =
                                duration_ms_from_mono_s16le(bytes.len(), WEBRTC_PCM_SAMPLE_RATE);
                            let pcm = mono_s16le_to_stereo(&bytes);
                            if s.send(TtsAudioChunk { pcm, duration_ms }) {
                                emitted_any = true;
                            }
                        }
                    }
                }
                // timestamps, phoneme_timestamps, flush_done, ...
                _ => {}
            }
        }
        Ok(WsOutcome {
            mono,
            emitted_any,
            cancelled: false,
        })
    }
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
