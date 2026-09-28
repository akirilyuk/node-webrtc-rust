use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::stt::AwsStt;
use crate::tts::AwsTts;

pub struct AwsFactory;

impl VendorFactory for AwsFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(AwsStt::new(config)?))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(AwsTts::new(config)?))
    }
}
