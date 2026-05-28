use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::SttConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

use crate::factory::{api_key_from, OpenAiSttState, SharedSttState};

pub struct OpenAiStt {
    api_key: Option<String>,
    state: SharedSttState,
}

impl OpenAiStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        let api_key = config.api_key.clone().or_else(|| std::env::var("OPENAI_API_KEY").ok());
        Ok(Self {
            api_key,
            state: std::sync::Arc::new(tokio::sync::Mutex::new(OpenAiSttState {
                buffered: Vec::new(),
                running: false,
            })),
        })
    }
}

#[async_trait]
impl SttProvider for OpenAiStt {
    fn vendor_name(&self) -> &'static str {
        "openai"
    }

    async fn start(&mut self) -> SpeechResult<()> {
        self.state.lock().await.running = true;
        Ok(())
    }

    async fn stop(&mut self) -> SpeechResult<()> {
        self.state.lock().await.running = false;
        Ok(())
    }

    async fn push_audio(&mut self, pcm: Bytes) -> SpeechResult<()> {
        self.state.lock().await.buffered.extend_from_slice(pcm.as_ref());
        Ok(())
    }

    async fn poll_transcript(&mut self) -> SpeechResult<Option<SttTranscript>> {
        #[cfg(feature = "live")]
        {
            let (pcm, api_key) = {
                let mut state = self.state.lock().await;
                if state.buffered.len() < 3200 || !state.running {
                    return Ok(None);
                }
                let pcm = Bytes::from(std::mem::take(&mut state.buffered));
                let api_key = api_key_from(&self.api_key, "OPENAI_API_KEY")?;
                (pcm, api_key)
            };
            return live_transcribe(pcm, &api_key).await.map(Some);
        }

        #[cfg(not(feature = "live"))]
        {
            let _ = api_key_from(&self.api_key, "OPENAI_API_KEY");
            Err(SpeechError::Vendor {
                vendor: "openai".into(),
                message: "live OpenAI STT requires `--features live` on vendor-openai".into(),
            })
        }
    }
}

#[cfg(feature = "live")]
async fn live_transcribe(pcm: Bytes, api_key: &str) -> SpeechResult<SttTranscript> {
    use async_openai::types::AudioInput;
    use async_openai::Client;

    let client = Client::with_config(async_openai::config::OpenAIConfig::new().with_api_key(api_key));
    let _ = (client, pcm);
    Err(SpeechError::Vendor {
        vendor: "openai".into(),
        message: "OpenAI live STT wiring pending".into(),
    })
}
