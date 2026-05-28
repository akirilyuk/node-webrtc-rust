use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

pub struct GoogleStt {
    running: bool,
}

impl GoogleStt {
    pub fn new(_config: &SttConfig) -> SpeechResult<Self> {
        Ok(Self { running: false })
    }
}

#[async_trait]
impl SttProvider for GoogleStt {
    fn vendor_name(&self) -> &'static str {
        "google"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.running = false;
        Ok(())
    }

    async fn push_audio(&mut self, _pcm: Bytes) -> SpeechResult<()> {
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        if !self.running {
            return Ok(None);
        }
        Err(SpeechError::Vendor {
            vendor: "google".into(),
            message: "Google Cloud Speech live STT wiring pending".into(),
        })
    }
}
