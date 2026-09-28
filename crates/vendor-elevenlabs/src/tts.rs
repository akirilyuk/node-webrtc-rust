use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{
    duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use crate::matrix::{
    http_stream_url, tts_delivery_plan, tts_progressive_transport, tts_websocket_allowed,
    websocket_stream_input_url, DEFAULT_MODEL_ID, TtsDeliveryPlan, TtsProgressiveTransport,
};

const DEFAULT_VOICE_ID: &str = "EXAVITQu4vr4xnSDxMaL";

pub struct ElevenLabsTts {
    api_key: Option<String>,
    model: String,
    voice: Option<String>,
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
            TtsProgressiveTransport::HttpStream => {
                self.synthesize_http_stream(text, sink).await
            }
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
            let url = format!(
                "https://api.elevenlabs.io/v1/text-to-speech/{voice_id}?output_format=pcm_48000"
            );
            let body = serde_json::json!({
                "text": text,
                "model_id": self.model,
            });

            let response = reqwest::Client::new()
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
            return Ok(vec![pcm_chunk_from_mono(mono.as_ref())]);
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
        let url = http_stream_url(&voice_id);
        let body = serde_json::json!({
            "text": text,
            "model_id": self.model,
        });

        let response = reqwest::Client::new()
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

        if !response.status().is_success() {
            let status = response.status();
            let message = response.text().await.unwrap_or_else(|_| status.to_string());
            return Err(SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message,
            });
        }

        let mut stream = response.bytes_stream();
        let mut mono = Vec::new();
        while let Some(item) = stream.next().await {
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                break;
            }
            let chunk = item.map_err(|err| SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: err.to_string(),
            })?;
            mono.extend_from_slice(&chunk);
            if let Some(s) = sink.as_ref() {
                let audio = pcm_chunk_from_mono(&chunk);
                let _ = s.send(audio);
            }
        }

        if mono.is_empty() && matches!(tts_delivery_plan(&self.model), TtsDeliveryPlan::HttpStream)
        {
            return self.synthesize_full_body(text).await;
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

    #[cfg(feature = "live")]
    async fn synthesize_websocket_stream(
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
        let url = websocket_stream_input_url(&voice_id, &self.model);
        let mut request = url.into_client_request().map_err(|err| SpeechError::Vendor {
            vendor: "elevenlabs".into(),
            message: err.to_string(),
        })?;
        request
            .headers_mut()
            .insert("xi-api-key", api_key.parse().unwrap());

        let (ws, _) = connect_async(request)
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: err.to_string(),
            })?;
        let (mut ws_tx, mut ws_rx) = ws.split();

        ws_tx
            .send(Message::Text(
                serde_json::json!({
                    "text": " ",
                    "voice_settings": {"stability": 0.5, "similarity_boost": 0.8, "use_speaker_boost": false},
                })
                .to_string()
                .into(),
            ))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: err.to_string(),
            })?;
        ws_tx
            .send(Message::Text(
                serde_json::json!({"text": text, "flush": true}).to_string().into(),
            ))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: err.to_string(),
            })?;
        ws_tx
            .send(Message::Text(serde_json::json!({"text": ""}).to_string().into()))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: err.to_string(),
            })?;

        let mut mono = Vec::new();
        while let Some(msg) = ws_rx.next().await {
            if sink.as_ref().is_some_and(TtsProgressiveSink::is_cancelled) {
                break;
            }
            let Ok(Message::Text(raw)) = msg else { break };
            let value: serde_json::Value = serde_json::from_str(&raw).map_err(|err| {
                SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: err.to_string(),
                }
            })?;
            if value.get("isFinal").and_then(|v| v.as_bool()) == Some(true) {
                break;
            }
            if let Some(audio_b64) = value.get("audio").and_then(|v| v.as_str()) {
                let bytes = STANDARD.decode(audio_b64).map_err(|err| SpeechError::Vendor {
                    vendor: "elevenlabs".into(),
                    message: err.to_string(),
                })?;
                mono.extend_from_slice(&bytes);
                if let Some(s) = sink.as_ref() {
                    let _ = s.send(pcm_chunk_from_mono(&bytes));
                }
            }
        }
        Ok(vec![pcm_chunk_from_mono(&mono)])
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
