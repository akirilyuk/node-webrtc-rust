use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProvider};

use crate::client::CartesiaClient;

pub struct CartesiaTts {
    client: CartesiaClient,
    voice: Option<String>,
}

impl CartesiaTts {
    pub fn new(config: &TtsConfig) -> SpeechResult<Self> {
        Ok(Self {
            client: CartesiaClient::new(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("CARTESIA_API_KEY").ok()),
            ),
            voice: config.voice.clone(),
        })
    }
}

pub fn unsupported_stt(_config: &SttConfig) -> SpeechResult<Box<dyn node_webrtc_rust_speech::pipeline::SttProvider>> {
    Err(SpeechError::Config("Cartesia does not provide STT".into()))
}

#[async_trait]
impl TtsProvider for CartesiaTts {
    fn vendor_name(&self) -> &'static str {
        "cartesia"
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        let voice = self.voice.as_deref().unwrap_or("default");
        let _pcm = self.client.synthesize_text(text, voice).await?;
        Ok(vec![TtsAudioChunk {
            pcm: Bytes::new(),
            duration_ms: 20,
        }])
    }
}
