use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo, WEBRTC_PCM_SAMPLE_RATE};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

#[cfg(feature = "live")]
use crate::auth::{google_access_token, google_api_key};

pub struct GoogleTts {
    voice: String,
    language: String,
}

impl GoogleTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let voice = config
            .voice
            .clone()
            .or_else(|| config.model.clone())
            .unwrap_or_else(|| "en-US-Neural2-A".to_string());
        let language = voice
            .split('-')
            .take(2)
            .collect::<Vec<_>>()
            .join("-");
        Ok(Self {
            voice,
            language: if language.is_empty() {
                "en-US".to_string()
            } else {
                language
            },
        })
    }
}

#[async_trait]
impl TtsProvider for GoogleTts {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let mono = synthesize_linear16(text, &self.voice, &self.language).await?;
            let duration_ms = duration_ms_from_mono_s16le(mono.len(), WEBRTC_PCM_SAMPLE_RATE);
            let pcm = mono_s16le_to_stereo(&mono);
            return Ok(vec![TtsAudioChunk { pcm, duration_ms }]);
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = (text, &self.voice);
            Err(SpeechError::Vendor {
                vendor: "google".into(),
                message: "live Google TTS requires `--features live` on vendor-google".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
async fn synthesize_linear16(text: &str, voice: &str, language: &str) -> SpeechResult<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde_json::json;

    let client = reqwest::Client::new();
    let body = json!({
        "input": { "text": text },
        "voice": {
            "languageCode": language,
            "name": voice
        },
        "audioConfig": {
            "audioEncoding": "LINEAR16",
            "sampleRateHertz": WEBRTC_PCM_SAMPLE_RATE,
            "speakingRate": 1.0
        }
    });

    let request = if let Some(api_key) = google_api_key() {
        client.post(format!(
            "https://texttospeech.googleapis.com/v1/text:synthesize?key={api_key}"
        ))
    } else {
        let token = google_access_token()
            .await?
            .ok_or_else(|| SpeechError::Vendor {
                vendor: "google".into(),
                message: "failed to obtain Google access token".into(),
            })?;
        client.post("https://texttospeech.googleapis.com/v1/text:synthesize")
            .bearer_auth(token)
    };

    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|err| SpeechError::Vendor {
            vendor: "google".into(),
            message: err.to_string(),
        })?;

    let status = response.status();
    let payload: serde_json::Value = response.json().await.map_err(|err| SpeechError::Vendor {
        vendor: "google".into(),
        message: err.to_string(),
    })?;

    if !status.is_success() {
        return Err(SpeechError::Vendor {
            vendor: "google".into(),
            message: payload.to_string(),
        });
    }

    let audio_b64 = payload
        .get("audioContent")
        .and_then(|value| value.as_str())
        .ok_or_else(|| SpeechError::Vendor {
            vendor: "google".into(),
            message: "missing audioContent in TTS response".into(),
        })?;

    STANDARD.decode(audio_b64).map_err(|err| SpeechError::Vendor {
        vendor: "google".into(),
        message: err.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn derives_language_from_voice_name() {
        let tts = GoogleTts::new(&TtsConfig {
            provider: TtsVendor::Google,
            model: None,
            voice: Some("en-US-Neural2-A".into()),
            api_key: None,
        })
        .unwrap();
        assert_eq!(tts.language, "en-US");
        assert_eq!(tts.voice, "en-US-Neural2-A");
    }
}
