use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::stt::DeepgramStt;
use crate::tts::DeepgramTts;

pub struct DeepgramFactory;

impl VendorFactory for DeepgramFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(DeepgramStt::new(config)?))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(DeepgramTts::new(config)?))
    }
}
