use async_trait::async_trait;
use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

#[cfg(feature = "live")]
use crate::matrix::{
    full_body_url, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, FIRST_AUDIO_TIMEOUT, HTTP_TOTAL_TIMEOUT,
    POOL_IDLE_TIMEOUT, TCP_KEEPALIVE, WS_CONNECT_TIMEOUT,
};
use crate::matrix::{
    http_stream_url, resolve_tts_transport, tts_delivery_plan, tts_progressive_transport_for,
    tts_websocket_allowed, websocket_stream_input_url, TtsDeliveryPlan, TtsProgressiveTransport,
    DEFAULT_BASE_URL, DEFAULT_MODEL_ID,
};

const DEFAULT_VOICE_ID: &str = "EXAVITQu4vr4xnSDxMaL";

pub struct ElevenLabsTts {
    api_key: Option<String>,
    model: String,
    voice: Option<String>,
    /// Resolved customer transport (`http` = chunked HTTP, `websocket` = stream-input socket).
    transport: SpeechTransport,
    /// REST base (`TtsConfig.endpoint` or [`DEFAULT_BASE_URL`]); the WS host is derived from it.
    #[cfg_attr(not(feature = "live"), allow(dead_code))]
    base_url: String,
    /// One HTTP client for the provider's life: connections (and TLS sessions) are reused. HTTP
    /// version: ALPN default (h2 when the server offers it, else HTTP/1.1). It has no total
    /// timeout, because that would cut a long streamed utterance; the full-body request sets
    /// [`HTTP_TOTAL_TIMEOUT`] per request and the stream waits are bounded one by one.
    #[cfg(feature = "live")]
    http: reqwest::Client,
    /// How the last request reached ElevenLabs: `ws`, `h2` or `http/1.1`.
    last_transport: std::sync::Mutex<Option<String>>,
}

impl ElevenLabsTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string());
        let transport = resolve_tts_transport(&model, config.transport)?;
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok()),
            model,
            voice: config.voice.clone(),
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
                .pool_idle_timeout(POOL_IDLE_TIMEOUT)
                .tcp_keepalive(TCP_KEEPALIVE)
                .build()
                .map_err(|err| {
                    vendor_error(format!("elevenlabs http client build failed: {err}"))
                })?,
            last_transport: std::sync::Mutex::new(None),
        })
    }

    #[cfg(feature = "live")]
    fn note_http_version(&self, response: &reqwest::Response) {
        self.note_transport(if response.version() == reqwest::Version::HTTP_2 {
            "h2"
        } else {
            "http/1.1"
        });
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
            .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SpeechError::Config("missing ELEVENLABS_API_KEY".into()))
    }

    fn voice_id(&self) -> String {
        self.voice
            .clone()
            .filter(|voice| voice != "default")
            .or_else(|| std::env::var("ELEVENLABS_VOICE_ID").ok())
            .unwrap_or_else(|| DEFAULT_VOICE_ID.to_string())
    }
}

#[async_trait]
impl TtsProvider for ElevenLabsTts {
    fn vendor_name(&self) -> &'static str {
        "elevenlabs"
    }

    fn last_transport(&self) -> Option<String> {
        self.last_transport.lock().ok().and_then(|g| g.clone())
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_full_body(text).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        match tts_progressive_transport_for(&self.model, self.transport) {
            TtsProgressiveTransport::FullBodyPost => {
                let chunks = self.synthesize_full_body(text).await?;
                emit_chunks(sink.as_ref(), &chunks);
                Ok(chunks)
            }
            TtsProgressiveTransport::HttpStream => self.synthesize_http_stream(text, sink).await,
            TtsProgressiveTransport::WebSocketStreamInput => {
                #[cfg(feature = "live")]
                {
                    if tts_websocket_allowed(&self.model) {
                        return self.synthesize_websocket_stream(text, sink).await;
                    }
                }
                self.synthesize_http_stream(text, sink).await
            }
        }
    }
}

impl ElevenLabsTts {
    async fn synthesize_full_body(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let api_key = self.api_key()?;
            let voice_id = self.voice_id();
            let url = full_body_url(&self.base_url, &voice_id);
            let body = serde_json::json!({
                "text": text,
                "model_id": self.model,
            });

            let response = self
                .http
                .post(url)
                .timeout(HTTP_TOTAL_TIMEOUT)
                .header("xi-api-key", api_key)
                .header("Content-Type", "application/json")
                .header("Accept", "audio/pcm")
                .json(&body)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: err.to_string(),
                })?;

            self.note_http_version(&response);
            let status = response.status();
            if !status.is_success() {
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: http_status_message(status.as_u16(), &message),
                });
            }

            let mono = response.bytes().await.map_err(|err| SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: err.to_string(),
            })?;
            Ok(vec![pcm_chunk_from_mono(mono.as_ref())])
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, self.api_key()?);
            Err(SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: "live ElevenLabs TTS requires `--features live` on vendor-elevenlabs"
                    .into(),
            })
        }
    }

    #[cfg(feature = "live")]
    async fn synthesize_http_stream(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        use futures_util::StreamExt;

        let api_key = self.api_key()?;
        let voice_id = self.voice_id();
        let url = http_stream_url(&self.base_url, &voice_id);
        let body = serde_json::json!({
            "text": text,
            "model_id": self.model,
        });

        // No total timeout here: it would cut a long utterance. Each wait is bounded instead.
        let first_audio_timed_out = || {
            vendor_error(format!(
                "elevenlabs http first audio timed out after {} ms",
                FIRST_AUDIO_TIMEOUT.as_millis()
            ))
        };
        // One deadline covers the response headers and the first audio chunk together.
        let first_audio_deadline = tokio::time::Instant::now() + FIRST_AUDIO_TIMEOUT;
        let response = tokio::time::timeout_at(
            first_audio_deadline,
            self.http
                .post(url)
                .header("xi-api-key", api_key)
                .header("Content-Type", "application/json")
                .header("Accept", "audio/pcm")
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| first_audio_timed_out())?
        .map_err(|err| vendor_error(err.to_string()))?;

        self.note_http_version(&response);
        if !response.status().is_success() {
            let status = response.status();
            let message = match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, response.text()).await {
                Ok(Ok(text)) => text,
                _ => status.to_string(),
            };
            return Err(vendor_error(http_status_message(status.as_u16(), &message)));
        }

        let mut stream = response.bytes_stream();
        let mut mono = Vec::new();
        let mut framer = WholeMsFramer::default();
        let mut cancelled = false;
        loop {
            let next = if mono.is_empty() && framer.is_empty() {
                tokio::time::timeout_at(first_audio_deadline, stream.next())
                    .await
                    .map_err(|_| first_audio_timed_out())?
            } else {
                tokio::time::timeout(CHUNK_IDLE_TIMEOUT, stream.next())
                    .await
                    .map_err(|_| {
                        vendor_error(format!(
                            "elevenlabs http stalled after {} ms",
                            CHUNK_IDLE_TIMEOUT.as_millis()
                        ))
                    })?
            };
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                cancelled = true;
                break;
            }
            let Some(item) = next else { break };
            let chunk = item.map_err(|err| vendor_error(err.to_string()))?;
            let Some(bytes) = framer.push(&chunk) else {
                continue;
            };
            mono.extend_from_slice(&bytes);
            if let Some(s) = sink.as_ref() {
                let audio = pcm_chunk_from_mono(&bytes);
                let _ = s.send(audio);
            }
        }
        let (tail, odd_byte) = framer.finish();
        if !cancelled && !tail.is_empty() {
            mono.extend_from_slice(&tail);
            if let Some(s) = sink.as_ref() {
                let _ = s.send(pcm_chunk_from_mono(&tail));
            }
        }
        if odd_byte && !cancelled {
            // A stream that ends mid-sample: the half sample cannot be played.
            if std::env::var("VOICE_DEBUG")
                .is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            {
                eprintln!(
                    "[voice-debug] elevenlabs http stream ended on an odd byte; dropped 1 byte"
                );
            }
        }

        if cancelled {
            // A cancelled utterance must not be re-synthesized by the full-body fallback.
            return Ok(vec![pcm_chunk_from_mono(&mono)]);
        }
        if mono.is_empty() && matches!(tts_delivery_plan(&self.model), TtsDeliveryPlan::HttpStream)
        {
            // The stream produced no audio; the full body's audio reaches the sink once, here.
            let chunks = self.synthesize_full_body(text).await?;
            emit_chunks(sink.as_ref(), &chunks);
            return Ok(chunks);
        }
        Ok(vec![pcm_chunk_from_mono(&mono)])
    }

    #[cfg(not(feature = "live"))]
    async fn synthesize_http_stream(
        &self,
        _text: &str,
        _sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        Err(SpeechError::Vendor {
            vendor: "elevenlabs".into(),
            message: "live ElevenLabs TTS requires `--features live` on vendor-elevenlabs".into(),
        })
    }

    /// Direct ElevenLabs stream-input WebSocket synthesis. Selected by the `websocket` customer
    /// transport; public so the mock-server tests can exercise it directly.
    #[cfg(feature = "live")]
    pub async fn synthesize_websocket_stream(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let api_key = self.api_key()?;
        let voice_id = self.voice_id();
        let url = websocket_stream_input_url(&self.base_url, &voice_id, &self.model);
        let ws = crate::ws_connect::connect(url, &api_key, "tts").await?;
        self.note_transport("ws");
        let (mut ws_tx, mut ws_rx) = ws.split();
        let connect_timeout = || {
            vendor_error(format!(
                "elevenlabs tts websocket send timed out after {} s",
                WS_CONNECT_TIMEOUT.as_secs()
            ))
        };

        tokio::time::timeout(
            WS_CONNECT_TIMEOUT,
            ws_tx.send(Message::Text(
                serde_json::json!({
                    "text": " ",
                    "voice_settings": {"stability": 0.5, "similarity_boost": 0.8, "use_speaker_boost": false},
                })
                .to_string()
                .into(),
            )),
        )
        .await
        .map_err(|_| connect_timeout())?
        .map_err(|err| vendor_error(err.to_string()))?;
        tokio::time::timeout(
            WS_CONNECT_TIMEOUT,
            ws_tx.send(Message::Text(
                // Docs (SendText): the text should "always end with a single space string".
                serde_json::json!({"text": format!("{} ", text.trim_end()), "flush": true})
                    .to_string()
                    .into(),
            )),
        )
        .await
        .map_err(|_| connect_timeout())?
        .map_err(|err| vendor_error(err.to_string()))?;
        tokio::time::timeout(
            WS_CONNECT_TIMEOUT,
            ws_tx.send(Message::Text(
                serde_json::json!({"text": ""}).to_string().into(),
            )),
        )
        .await
        .map_err(|_| connect_timeout())?
        .map_err(|err| vendor_error(err.to_string()))?;

        let mut mono = Vec::new();
        loop {
            let (bound, bound_message) = if mono.is_empty() {
                (FIRST_AUDIO_TIMEOUT, "first audio timed out")
            } else {
                (CHUNK_IDLE_TIMEOUT, "stalled")
            };
            let next = tokio::time::timeout(bound, ws_rx.next())
                .await
                .map_err(|_| {
                    vendor_error(format!(
                        "elevenlabs ws {bound_message} after {} ms",
                        bound.as_millis()
                    ))
                })?;
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                let _ = tokio::time::timeout(WS_CONNECT_TIMEOUT, ws_tx.close()).await;
                return Ok(vec![pcm_chunk_from_mono(&mono)]);
            }
            let raw = match next {
                None => return Err(vendor_error("elevenlabs ws closed before isFinal")),
                Some(Ok(Message::Close(frame))) => {
                    // The close reason is WebSocket protocol, not an ElevenLabs field.
                    let reason = frame
                        .map(|f| f.reason.to_string())
                        .filter(|r| !r.is_empty());
                    return Err(vendor_error(match reason {
                        Some(reason) => format!("elevenlabs ws closed before isFinal: {reason}"),
                        None => "elevenlabs ws closed before isFinal".to_string(),
                    }));
                }
                Some(Err(err)) => {
                    return Err(vendor_error(format!("elevenlabs ws read failed: {err}")))
                }
                Some(Ok(Message::Text(raw))) => raw,
                // Ping/Pong are answered by tungstenite; Binary/Frame are not part of this API.
                Some(Ok(_)) => continue,
            };
            let value: serde_json::Value =
                serde_json::from_str(&raw).map_err(|err| vendor_error(err.to_string()))?;
            if value.get("isFinal").and_then(|v| v.as_bool()) == Some(true) {
                break;
            }
            if let Some(audio_b64) = value.get("audio").and_then(|v| v.as_str()) {
                let bytes = STANDARD
                    .decode(audio_b64)
                    .map_err(|err| vendor_error(err.to_string()))?;
                mono.extend_from_slice(&bytes);
                if let Some(s) = sink.as_ref() {
                    let _ = s.send(pcm_chunk_from_mono(&bytes));
                }
            }
        }
        Ok(vec![pcm_chunk_from_mono(&mono)])
    }
}

#[cfg(feature = "live")]
fn vendor_error(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "elevenlabs".into(),
        message: message.into(),
    }
}

/// Error text of a non-2xx answer: `<numeric status>: <body>`, so callers can read the status.
#[cfg_attr(not(feature = "live"), allow(dead_code))]
fn http_status_message(status: u16, body: &str) -> String {
    format!("{status}: {body}")
}

/// Mono s16le bytes in one millisecond at [`WEBRTC_PCM_SAMPLE_RATE`] (48 samples of 2 bytes).
const MONO_BYTES_PER_MS: usize = (WEBRTC_PCM_SAMPLE_RATE as usize / 1000) * 2;

/// Cuts the HTTP body into whole milliseconds of audio before it becomes sink chunks.
///
/// The body arrives in network-sized pieces of any length. A [`TtsAudioChunk`] declares whole
/// milliseconds (`duration_ms_from_mono_s16le` rounds down), so a piece of 1.9 ms declared 1 ms:
/// over a stream of small pieces the declared durations fell short of the audio by several
/// percent (catalog `audio.sample-rate-mismatch`). Emitting only whole milliseconds and carrying
/// the rest makes every chunk's `duration_ms` exact; only the final tail can be under 1 ms.
/// An odd byte is the first half of the next sample and is carried the same way.
#[derive(Default)]
#[cfg_attr(not(feature = "live"), allow(dead_code))]
struct WholeMsFramer {
    pending: Vec<u8>,
}

#[cfg_attr(not(feature = "live"), allow(dead_code))]
impl WholeMsFramer {
    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Adds one body piece; returns the whole milliseconds now available, if any.
    fn push(&mut self, piece: &[u8]) -> Option<Vec<u8>> {
        self.pending.extend_from_slice(piece);
        let whole = self.pending.len() - self.pending.len() % MONO_BYTES_PER_MS;
        if whole == 0 {
            return None;
        }
        let rest = self.pending.split_off(whole);
        Some(std::mem::replace(&mut self.pending, rest))
    }

    /// End of stream: the remaining whole samples (under 1 ms), and whether an odd byte (half a
    /// sample, unplayable) was dropped.
    fn finish(self) -> (Vec<u8>, bool) {
        let mut tail = self.pending;
        let odd_byte = tail.len() % 2 == 1;
        if odd_byte {
            tail.pop();
        }
        (tail, odd_byte)
    }
}

fn pcm_chunk_from_mono(mono: &[u8]) -> TtsAudioChunk {
    let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
    let pcm = mono_s16le_to_stereo(mono);
    TtsAudioChunk { pcm, duration_ms }
}

fn emit_chunks(sink: Option<&TtsProgressiveSink>, chunks: &[TtsAudioChunk]) {
    if let Some(s) = sink {
        for chunk in chunks {
            if !s.send(chunk.clone()) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    /// Splits `total` mono bytes into the uneven network pieces of a streamed HTTP body.
    fn uneven_pieces(total: usize) -> Vec<Vec<u8>> {
        // 14-16 ms pieces that end just short of a whole millisecond (one odd-length).
        let sizes = [1438usize, 1342, 1534, 1247, 1439];
        let mut pieces = Vec::new();
        let mut sent = 0;
        let mut i = 0;
        while sent < total {
            let n = sizes[i % sizes.len()].min(total - sent);
            pieces.push(vec![(i % 251) as u8; n]);
            sent += n;
            i += 1;
        }
        pieces
    }

    /// The live failure: 793564 stereo bytes (4133 ms) declared 4046 ms because each network
    /// piece rounded its duration down. Through the framer the declared sum matches the bytes.
    #[test]
    fn uneven_http_pieces_declare_the_duration_of_their_bytes() {
        let total_mono = 793_564 / 2;
        let pieces = uneven_pieces(total_mono);

        // Before: one chunk per network piece.
        let naive: u64 = pieces
            .iter()
            .map(|p| u64::from(pcm_chunk_from_mono(&p[..p.len() - p.len() % 2]).duration_ms))
            .sum();
        assert!(
            (naive as f64 - 4133.0).abs() > 4133.0 * 0.02,
            "the repro must reproduce the > 2 % shortfall, got {naive} ms"
        );

        let mut framer = WholeMsFramer::default();
        let mut chunks = Vec::new();
        for piece in &pieces {
            if let Some(bytes) = framer.push(piece) {
                assert_eq!(bytes.len() % MONO_BYTES_PER_MS, 0);
                chunks.push(pcm_chunk_from_mono(&bytes));
            }
        }
        let (tail, odd) = framer.finish();
        assert!(!odd);
        if !tail.is_empty() {
            assert!(tail.len() < MONO_BYTES_PER_MS);
            chunks.push(pcm_chunk_from_mono(&tail));
        }
        let declared: u64 = chunks.iter().map(|c| u64::from(c.duration_ms)).sum();
        let bytes: usize = chunks.iter().map(|c| c.pcm.len()).sum();
        assert_eq!(bytes, 793_564, "no audio lost or duplicated");
        let from_bytes = bytes as f64 / 192.0;
        assert!(
            (declared as f64 - from_bytes).abs() <= 1.0,
            "declared {declared} ms vs {from_bytes:.1} ms of bytes"
        );
    }

    #[test]
    fn framer_carries_an_odd_byte_and_drops_it_only_at_the_end() {
        let mut framer = WholeMsFramer::default();
        assert_eq!(framer.push(&[1u8; 95]), None);
        let out = framer.push(&[2u8; 2]).expect("one whole ms");
        assert_eq!(out.len(), MONO_BYTES_PER_MS);
        assert_eq!(&out[94..], &[1, 2], "the odd byte stays in sample order");
        let (tail, odd) = framer.finish();
        assert!(tail.is_empty());
        assert!(odd, "a stream ending mid-sample drops one byte");

        let mut framer = WholeMsFramer::default();
        assert_eq!(framer.push(&[3u8; 40]), None);
        let (tail, odd) = framer.finish();
        assert_eq!(tail.len(), 40, "a sub-ms tail is still emitted");
        assert!(!odd);
    }

    #[test]
    fn resolves_default_voice_id() {
        let tts = ElevenLabsTts::new(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
            model: None,
            model_path: None,
            voice: None,
            api_key: Some("test".into()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        })
        .unwrap();
        assert_eq!(tts.voice_id(), DEFAULT_VOICE_ID);
    }

    #[test]
    fn eleven_v3_uses_full_body_plan() {
        assert_eq!(
            tts_delivery_plan("eleven_v3"),
            TtsDeliveryPlan::FullBodyPost
        );
    }
}

#[cfg(test)]
mod http_status_message_tests {
    use super::http_status_message;

    #[test]
    fn error_text_starts_with_the_numeric_status() {
        let text = http_status_message(429, "slow down");
        assert_eq!(text, "429: slow down");
    }
}
