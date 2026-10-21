use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::tts::MistralTts;

pub struct MistralFactory;

impl VendorFactory for MistralFactory {
    fn create_stt(&self, _config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        // Realtime STT is added in a later change.
        Err(SpeechError::Config(
            "mistral stt is not implemented yet".into(),
        ))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(MistralTts::new(config)?))
    }
}
