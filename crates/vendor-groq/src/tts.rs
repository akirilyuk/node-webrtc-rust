//! Groq TTS — documented `POST /openai/v1/audio/speech` (full body; no documented SSE stream).

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

use node_webrtc_rust_voice_catalog as voice_catalog;

use crate::factory::api_key_from;
use crate::matrix::{default_tts_model, validate_tts_model, DEFAULT_SPEECH_URL};

const GROQ_TTS_PCM_RATE: u32 = 24_000;

pub struct GroqTts {
    api_key: Option<String>,
    speech_url: String,
    model: String,
    voice: String,
}

impl GroqTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        let model = config
            .model
            .clone()
            .unwrap_or_else(|| default_tts_model().to_string());
        validate_tts_model(&model).map_err(SpeechError::Config)?;
        let documented = voice_catalog::tts_voices_for_model("groq", Some(&model))
            .expect("groq in voice catalog");
        let voice = match config.voice.as_deref().filter(|v| !v.is_empty()) {
            Some(voice) => {
                if !documented.contains(&voice) {
                    return Err(SpeechError::Config(format!(
                        "unsupported Groq voice `{voice}` for model `{model}`; documented voices: {}",
                        documented.join(", ")
                    )));
                }
                voice.to_string()
            }
            None => match voice_catalog::default_tts_voice("groq")
                .filter(|default| documented.contains(default))
            {
                Some(default) => default.to_string(),
                None => {
                    return Err(SpeechError::Config(format!(
                        "voice required for groq model `{model}`; documented voices: {}",
                        documented.join(", ")
                    )))
                }
            },
        };
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("GROQ_API_KEY").ok()),
            speech_url: config
                .endpoint
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_SPEECH_URL.to_string()),
            model,
            voice,
        })
    }
}

#[async_trait]
impl TtsProvider for GroqTts {
    fn vendor_name(&self) -> &'static str {
        "groq"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_full_body(text).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        let chunks = self.synthesize_full_body(text).await?;
        if let Some(s) = sink.as_ref() {
            for chunk in &chunks {
                let _ = s.send(chunk.clone());
            }
        }
        Ok(chunks)
    }
}

impl GroqTts {
    async fn synthesize_full_body(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        #[cfg(feature = "live")]
        {
            let api_key = api_key_from(&self.api_key, "GROQ_API_KEY")?;
            let body = serde_json::json!({
                "model": self.model,
                "input": text,
                "voice": self.voice,
                "response_format": "wav",
            });
            let response = reqwest::Client::new()
                .post(&self.speech_url)
                .bearer_auth(api_key)
                .json(&body)
                .send()
                .await
                .map_err(|err| SpeechError::Vendor {
                    vendor: "groq".into(),
                    message: err.to_string(),
                })?;
            if !response.status().is_success() {
                let status = response.status();
                let message = response.text().await.unwrap_or_else(|_| status.to_string());
                return Err(SpeechError::Vendor {
                    vendor: "groq".into(),
                    message,
                });
            }
            let wav = response.bytes().await.map_err(|err| SpeechError::Vendor {
                vendor: "groq".into(),
                message: err.to_string(),
            })?;
            let mono = wav_pcm_payload(&wav).map_err(SpeechError::Config)?;
            let duration_ms = duration_ms_from_mono_s16le(mono.len(), GROQ_TTS_PCM_RATE);
            let pcm = mono_s16le_to_stereo(&mono);
            Ok(vec![TtsAudioChunk { pcm, duration_ms }])
        }
        #[cfg(not(feature = "live"))]
        {
            let _ = text;
            Err(SpeechError::Vendor {
                vendor: "groq".into(),
                message: "live Groq TTS requires `--features live` on vendor-groq".into(),
            })
        }
    }
}

fn wav_pcm_payload(wav: &[u8]) -> Result<Vec<u8>, String> {
    if wav.len() < 44 || !wav.starts_with(b"RIFF") {
        return Err("Groq TTS response is not a WAV file".into());
    }
    Ok(wav[44..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use node_webrtc_rust_speech::config::TtsVendor;

    #[test]
    fn rejects_undocumented_tts_model() {
        assert!(matches!(
            GroqTts::new(&TtsConfig {
                provider: TtsVendor::Groq,
                model: Some("tts-1".into()),
                model_path: None,
                voice: None,
                api_key: Some("k".into()),
                endpoint: None,
                sentence_case: Default::default(),
            }),
            Err(SpeechError::Config(_))
        ));
    }

    fn cfg(model: &str, voice: Option<&str>) -> TtsConfig {
        TtsConfig {
            provider: TtsVendor::Groq,
            model: Some(model.into()),
            model_path: None,
            voice: voice.map(str::to_string),
            api_key: Some("k".into()),
            endpoint: None,
            sentence_case: Default::default(),
        }
    }

    #[test]
    fn groq_default_voice_for_english_is_troy() {
        let tts = GroqTts::new(&cfg("canopylabs/orpheus-v1-english", None)).unwrap();
        assert_eq!(tts.voice, "troy");
    }

    #[test]
    fn groq_arabic_without_voice_is_config_error() {
        assert!(matches!(
            GroqTts::new(&cfg("canopylabs/orpheus-arabic-saudi", None)),
            Err(SpeechError::Config(_))
        ));
    }

    #[test]
    fn groq_rejects_english_voice_on_arabic_model() {
        assert!(matches!(
            GroqTts::new(&cfg("canopylabs/orpheus-arabic-saudi", Some("troy"))),
            Err(SpeechError::Config(_))
        ));
    }
}
