use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

const DEFAULT_VOICE_ID: &str = "EXAVITQu4vr4xnSDxMaL";
const DEFAULT_MODEL_ID: &str = "eleven_multilingual_v2";

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
            let duration_ms =
                duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
            let pcm = mono_s16le_to_stereo(mono.as_ref());
            return Ok(vec![TtsAudioChunk { pcm, duration_ms }]);
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = self.api_key()?;
            Err(SpeechError::Vendor {
                vendor: "elevenlabs".into(),
                message: "live ElevenLabs TTS requires `--features live` on vendor-elevenlabs".into(),
            })
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
            voice: None,
            api_key: Some("test".into()),
        })
        .unwrap();
        assert_eq!(tts.voice_id(), DEFAULT_VOICE_ID);
    }
}
