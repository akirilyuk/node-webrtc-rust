//! Groq TTS — documented `POST /openai/v1/audio/speech` (full body; no documented SSE stream).

use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::{duration_ms_from_mono_s16le, mono_s16le_to_stereo};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider};

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
            voice: config.voice.clone().unwrap_or_else(|| "troy".to_string()),
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
            }),
            Err(SpeechError::Config(_))
        ));
    }
}
