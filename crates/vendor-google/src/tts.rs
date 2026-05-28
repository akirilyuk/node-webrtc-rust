use async_trait::async_trait;
use node_webrtc_rust_speech::config::TtsConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

pub struct GoogleTts {
    voice: Option<String>,
}

impl GoogleTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            voice: config.voice.clone(),
        })
    }
}

#[async_trait]
impl TtsProvider for GoogleTts {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let _ = (text, self.voice.as_deref());
        Err(SpeechError::Vendor {
            vendor: "google".into(),
            message: "Google Cloud TTS live wiring pending".into(),
        })
    }
}
