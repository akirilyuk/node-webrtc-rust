use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::stt::AssemblyAiStt;

pub struct AssemblyAiFactory;

impl VendorFactory for AssemblyAiFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(AssemblyAiStt::new(config)?))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        crate::stt::unsupported_tts(config)
    }
}
