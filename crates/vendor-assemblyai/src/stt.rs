use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};

use crate::client::AssemblyAiClient;
use crate::matrix::default_speech_model;

pub struct AssemblyAiStt {
    client: AssemblyAiClient,
    running: bool,
}

impl AssemblyAiStt {
    pub fn new(config: &SttConfig) -> SpeechResult<Self> {
        crate::matrix::check_stt_transport(config.transport)?;
        let speech_model = config
            .model
            .clone()
            .unwrap_or_else(|| default_speech_model().to_string());
        Ok(Self {
            client: AssemblyAiClient::new(
                config
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("ASSEMBLYAI_API_KEY").ok()),
                speech_model,
                config.endpoint.clone(),
            )?,
            running: false,
        })
    }
}

pub fn unsupported_tts(
    _config: &TtsConfig,
) -> SpeechResult<Box<dyn node_webrtc_rust_speech::pipeline::TtsProvider>> {
    Err(SpeechError::Config(
        "AssemblyAI does not provide TTS".into(),
    ))
}

#[async_trait]
impl SttProvider for AssemblyAiStt {
    fn vendor_name(&self) -> &'static str {
        "assemblyai"
    }

    fn last_transport(&self) -> Option<String> {
        self.client.last_transport()
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

    async fn finalize_utterance(&mut self) -> SpeechResult<()> {
        // Documented v3 `ForceEndpoint` ends the open turn and keeps the session, so the next
        // utterance reuses the socket. (`Terminate` also finalizes, but closes the session; stop()
        // sends it.)
        if !self.running {
            return Ok(());
        }
        self.client.force_endpoint().await
    }
}
