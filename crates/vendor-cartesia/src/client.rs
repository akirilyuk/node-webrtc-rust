//! Cartesia TTS REST client.

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};

const DEFAULT_API_VERSION: &str = "2024-11-13";

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

            return response.bytes().await.map(|bytes| bytes.to_vec()).map_err(|err| {
                SpeechError::Vendor {
                    vendor: "cartesia".into(),
                    message: err.to_string(),
                }
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
}
