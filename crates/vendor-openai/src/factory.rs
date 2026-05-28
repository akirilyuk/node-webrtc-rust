use std::sync::Arc;

use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::stt::OpenAiStt;
use crate::tts::OpenAiTts;

pub struct OpenAiFactory;

impl VendorFactory for OpenAiFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(OpenAiStt::new(config)?))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(OpenAiTts::new(config)?))
    }
}

pub(crate) fn api_key_from(config_key: &Option<String>, env: &str) -> SpeechResult<String> {
    if let Some(key) = config_key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }
    std::env::var(env).map_err(|_| {
        SpeechError::Config(format!("missing API key: set config.apiKey or {env}"))
    })
}

pub(crate) type SharedSttState = Arc<tokio::sync::Mutex<OpenAiSttState>>;

pub(crate) struct OpenAiSttState {
    pub buffered: Vec<u8>,
    pub running: bool,
    pub pending: Option<node_webrtc_rust_speech::pipeline::SttTranscript>,
}
