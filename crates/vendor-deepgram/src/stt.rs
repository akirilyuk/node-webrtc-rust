use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

pub struct DeepgramStt {
    api_key: Option<String>,
    running: bool,
}

impl DeepgramStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        Ok(Self {
            api_key: config
                .api_key
                .clone()
                .or_else(|| std::env::var("DEEPGRAM_API_KEY").ok()),
            running: false,
        })
    }
}

#[async_trait]
impl SttProvider for DeepgramStt {
    fn vendor_name(&self) -> &'static str {
        "deepgram"
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
        #[cfg(feature = "live")]
        {
            let _ = self.api_key.as_ref().ok_or_else(|| SpeechError::Config(
                "missing DEEPGRAM_API_KEY".into(),
            ))?;
            return Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "Deepgram live STT wiring pending".into(),
            });
        }
        #[cfg(not(feature = "live"))]
        {
            Err(SpeechError::Vendor {
                vendor: "deepgram".into(),
                message: "live Deepgram STT requires `--features live`".into(),
            })
        }
    }
}
