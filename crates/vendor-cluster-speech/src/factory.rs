use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::stt::ClusterSherpaStt;
use crate::tts::ClusterSherpaTts;

pub struct ClusterSherpaFactory;

impl VendorFactory for ClusterSherpaFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(ClusterSherpaStt::new(config)?))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(ClusterSherpaTts::new(config)?))
    }
}
