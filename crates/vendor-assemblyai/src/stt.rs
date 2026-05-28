use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

use crate::client::AssemblyAiClient;

pub struct AssemblyAiStt {
    client: AssemblyAiClient,
    running: bool,
}

impl AssemblyAiStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        Ok(Self {
            client: AssemblyAiClient::new(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("ASSEMBLYAI_API_KEY").ok()),
            ),
            running: false,
        })
    }
}

pub fn unsupported_tts(
    _config: &TtsConfig,
) -> SpeechResult<Box<dyn node_webrtc_rust_speech::pipeline::TtsProvider>> {
    Err(SpeechError::Config("AssemblyAI does not provide TTS".into()))
}

#[async_trait]
impl SttProvider for AssemblyAiStt {
    fn vendor_name(&self) -> &'static str {
        "assemblyai"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.client.connect().await?;
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.running = false;
        self.client.disconnect().await?;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        if !self.running {
            return Ok(());
        }
        self.client.push_audio(pcm).await
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        if !self.running {
            return Ok(None);
        }
        self.client.poll_transcript().await
    }
}
