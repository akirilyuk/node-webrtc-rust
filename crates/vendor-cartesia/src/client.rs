//! Cartesia TTS REST + WebSocket clients.
//!
//! REST bytes: <https://docs.cartesia.ai/api-reference/tts/bytes>
//! WebSocket: <https://docs.cartesia.ai/api-reference/tts/websocket>

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::TtsProgressiveSink;

use crate::matrix::{validate_model_id, websocket_url, DEFAULT_API_VERSION};

pub struct CartesiaClient {
    api_key: Option<String>,
    base_url: String,
    api_version: String,
}

impl CartesiaClient {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            base_url: "https://api.cartesia.ai".to_string(),
            api_version: DEFAULT_API_VERSION.to_string(),
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

            let response = reqwest::Client::new()
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

            let status = response.status();
            if !status.is_success() {
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "cartesia".into(),
                    message,
                });
            }

            return response
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(|err| SpeechError::Vendor {
                    vendor: "cartesia".into(),
                    message: err.to_string(),
                });
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, voice, model, &self.base_url);
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
    ) -> SpeechResult<Vec<u8>> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::{
            connect_async,
            tungstenite::{client::IntoClientRequest, Message},
        };

        validate_model_id(model).map_err(SpeechError::Config)?;
        let api_key = self.api_key()?;
        if voice == "default" {
            return Err(SpeechError::Config(
                "set tts.voice or CARTESIA_VOICE_ID to a Cartesia voice id".into(),
            ));
        }

        let context_id = format!(
            "ctx-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let url = websocket_url(&self.api_version);
        let mut request = url.into_client_request().map_err(|err| SpeechError::Vendor {
            vendor: "cartesia".into(),
            message: err.to_string(),
        })?;
        request
            .headers_mut()
            .insert("X-API-Key", api_key.parse().unwrap());
        request.headers_mut().insert(
            "Cartesia-Version",
            self.api_version.parse().unwrap(),
        );

        let (ws, _) = connect_async(request)
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "cartesia".into(),
                message: err.to_string(),
            })?;
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
        ws_tx
            .send(Message::Text(gen.to_string().into()))
            .await
            .map_err(|err| SpeechError::Vendor {
                vendor: "cartesia".into(),
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
                    vendor: "cartesia".into(),
                    message: err.to_string(),
                }
            })?;
            if value.get("type").and_then(|v| v.as_str()) == Some("done")
                && value.get("done").and_then(|v| v.as_bool()) == Some(true)
            {
                break;
            }
            if value.get("type").and_then(|v| v.as_str()) == Some("chunk") {
                if let Some(data_b64) = value.get("data").and_then(|v| v.as_str()) {
                    use base64::{engine::general_purpose::STANDARD, Engine as _};
                    let bytes = STANDARD.decode(data_b64).map_err(|err| SpeechError::Vendor {
                        vendor: "cartesia".into(),
                        message: err.to_string(),
                    })?;
                    mono.extend_from_slice(&bytes);
                    if let Some(s) = sink.as_ref() {
                        use node_webrtc_rust_speech::pcm::{
                            duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE,
                        };
                        use node_webrtc_rust_speech::pipeline::TtsAudioChunk;
                        let duration_ms =
                            duration_ms_from_mono_s16le(bytes.len(), WEBRTC_PCM_SAMPLE_RATE);
                        let pcm = mono_s16le_to_stereo(&bytes);
                        let _ = s.send(TtsAudioChunk { pcm, duration_ms });
                    }
                }
            }
        }
        Ok(mono)
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
