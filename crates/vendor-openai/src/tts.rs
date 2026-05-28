use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

use crate::factory::{api_key_from, stub_pcm};

pub struct OpenAiTts {
    api_key: Option<String>,
    voice: Option<String>,
}

impl OpenAiTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config.api_key.clone().or_else(|| std::env::var("OPENAI_API_KEY").ok()),
            voice: config.voice.clone(),
        })
    }
}

#[async_trait]
impl TtsProvider for OpenAiTts {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let _voice = self.voice.as_deref().unwrap_or("alloy");
        let _ = text;

        #[cfg(feature = "live")]
        {
            let _api_key = api_key_from(&self.api_key, "OPENAI_API_KEY")?;
            return Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "OpenAI live TTS wiring pending".into(),
            });
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = api_key_from(&self.api_key, "OPENAI_API_KEY");
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI TTS requires `--features live` on vendor-openai".into(),
            })
        }
    }
}

#[allow(dead_code)]
pub(crate) fn fallback_chunk(text: &str) -> TtsAudioChunk {
    let duration_ms = (text.len() as u32 * 40).clamp(100, 3000);
    stub_pcm(duration_ms)
}
