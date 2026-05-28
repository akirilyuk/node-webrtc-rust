use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

pub struct ElevenLabsTts {
    api_key: Option<String>,
    voice: Option<String>,
}

impl ElevenLabsTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok()),
            voice: config.voice.clone(),
        })
    }
}

#[async_trait]
impl TtsProvider for ElevenLabsTts {
    fn vendor_name(&self) -> &'static str {
        "elevenlabs"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let _ = text;
        let _voice = self.voice.as_deref().unwrap_or("default");
        if self.api_key.is_none() && std::env::var("ELEVENLABS_API_KEY").is_err() {
            return Err(SpeechError::Config("missing ELEVENLABS_API_KEY".into()));
        }
        Err(SpeechError::Vendor {
            vendor: "elevenlabs".into(),
            message: "ElevenLabs live TTS wiring pending (use mock vendor in CI)".into(),
        })
    }
}
