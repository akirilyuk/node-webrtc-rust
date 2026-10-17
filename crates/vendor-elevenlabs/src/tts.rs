use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

#[cfg(feature = "live")]
use crate::matrix::{
    full_body_url, CHUNK_IDLE_TIMEOUT, CONNECT_TIMEOUT, FIRST_AUDIO_TIMEOUT, HTTP_TOTAL_TIMEOUT,
};
use crate::matrix::{
    http_stream_url, tts_delivery_plan, tts_progressive_transport, tts_websocket_allowed,
    websocket_stream_input_url, TtsDeliveryPlan, TtsProgressiveTransport, DEFAULT_BASE_URL,
    DEFAULT_MODEL_ID,
};

const DEFAULT_VOICE_ID: &str = "EXAVITQu4vr4xnSDxMaL";

pub struct ElevenLabsTts {
    api_key: Option<String>,
    model: String,
    voice: Option<String>,
    /// REST base (`TtsConfig.endpoint` or [`DEFAULT_BASE_URL`]); the WS host is derived from it.
    #[cfg_attr(not(feature = "live"), allow(dead_code))]
    base_url: String,
}

impl ElevenLabsTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok()),
            model: config
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL_ID.to_string()),
            voice: config.voice.clone(),
            base_url: config
                .endpoint
                .as_deref()
                .map(str::trim)
                .map(|value| value.trim_end_matches('/'))
                .filter(|value| !value.is_empty())
                .unwrap_or(DEFAULT_BASE_URL)
                .to_string(),
        })
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

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_full_body(text).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        match tts_progressive_transport(&self.model) {
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

            let http = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(HTTP_TOTAL_TIMEOUT)
                .build()
                .map_err(|err| {
                    vendor_error(format!("elevenlabs http client build failed: {err}"))
                })?;
            let response = http
                .post(url)
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

            let status = response.status();
            if !status.is_success() {
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message,
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
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|err| vendor_error(format!("elevenlabs http client build failed: {err}")))?;
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
            http.post(url)
                .header("xi-api-key", api_key)
                .header("Content-Type", "application/json")
                .header("Accept", "audio/pcm")
                .json(&body)
                .send(),
        )
        .await
        .map_err(|_| first_audio_timed_out())?
        .map_err(|err| vendor_error(err.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let message = match tokio::time::timeout(CHUNK_IDLE_TIMEOUT, response.text()).await {
                Ok(Ok(text)) => text,
                _ => status.to_string(),
            };
            return Err(vendor_error(message));
        }

        let mut stream = response.bytes_stream();
        let mut mono = Vec::new();
        let mut cancelled = false;
        loop {
            let next = if mono.is_empty() {
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
            mono.extend_from_slice(&chunk);
            if let Some(s) = sink.as_ref() {
                let audio = pcm_chunk_from_mono(&chunk);
                let _ = s.send(audio);
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

    /// Direct ElevenLabs stream-input WebSocket synthesis. Not selected by
    /// `tts_progressive_transport` today; public so the mock-server tests can exercise it.
    #[cfg(feature = "live")]
    pub async fn synthesize_websocket_stream(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{
            connect_async,
            tungstenite::{client::IntoClientRequest, Message},
        };

        let api_key = self.api_key()?;
        let voice_id = self.voice_id();
        let url = websocket_stream_input_url(&self.base_url, &voice_id, &self.model);
        let mut request = url
            .into_client_request()
            .map_err(|err| vendor_error(err.to_string()))?;
        request.headers_mut().insert(
            "xi-api-key",
            api_key
                .parse()
                .map_err(|_| vendor_error("invalid ELEVENLABS_API_KEY header value"))?,
        );

        let connect_timeout = || {
            vendor_error(format!(
                "elevenlabs ws connect timed out after {} ms",
                CONNECT_TIMEOUT.as_millis()
            ))
        };
        let (ws, _) = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| connect_timeout())?
            .map_err(|err| vendor_error(err.to_string()))?;
        let (mut ws_tx, mut ws_rx) = ws.split();

        tokio::time::timeout(
            CONNECT_TIMEOUT,
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
            CONNECT_TIMEOUT,
            ws_tx.send(Message::Text(
                serde_json::json!({"text": text, "flush": true})
                    .to_string()
                    .into(),
            )),
        )
        .await
        .map_err(|_| connect_timeout())?
        .map_err(|err| vendor_error(err.to_string()))?;
        tokio::time::timeout(
            CONNECT_TIMEOUT,
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
                let _ = tokio::time::timeout(CONNECT_TIMEOUT, ws_tx.close()).await;
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
