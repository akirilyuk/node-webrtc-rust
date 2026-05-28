use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{
    SttProvider, SttTranscript, TtsAudioChunk, TtsProvider, VendorFactory,
};

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

pub(crate) fn stub_pcm(duration_ms: u32) -> TtsAudioChunk {
    let sample_rate = 48_000_u32;
    let samples = (sample_rate * duration_ms / 1000) as usize * 2;
    TtsAudioChunk {
        pcm: Bytes::from(vec![0_u8; samples * 2]),
        duration_ms,
    }
}

pub(crate) type SharedSttState = Arc<tokio::sync::Mutex<OpenAiSttState>>;

pub(crate) struct OpenAiSttState {
    pub buffered: Vec<u8>,
    pub running: bool,
}

#[async_trait]
pub(crate) trait LiveStt: Send + Sync {
    async fn transcribe(&self, pcm: Bytes, api_key: &str) -> SpeechResult<String>;
}

#[async_trait]
pub(crate) trait LiveTts: Send + Sync {
    async fn synthesize(&self, text: &str, api_key: &str) -> SpeechResult<Vec<TtsAudioChunk>>;
}
