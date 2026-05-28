use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::tts::CartesiaTts;

pub struct CartesiaFactory;

impl VendorFactory for CartesiaFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        crate::tts::unsupported_stt(config)
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(CartesiaTts::new(config)?))
    }
}
