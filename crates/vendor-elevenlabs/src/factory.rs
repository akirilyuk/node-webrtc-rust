use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::tts::ElevenLabsTts;

pub struct ElevenLabsFactory;

impl VendorFactory for ElevenLabsFactory {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Err(SpeechError::Config("ElevenLabs does not provide STT".into()))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(ElevenLabsTts::new(config)?))
    }
}
