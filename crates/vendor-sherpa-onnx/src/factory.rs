use node_webrtc_rust_speech::config::{LanguageIdConfig, SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::SpeechResult;
use node_webrtc_rust_speech::pipeline::{
    LanguageIdProvider, SttProvider, TtsProvider, VendorFactory,
};

use crate::lid::SherpaLanguageId;
use crate::stt::SherpaStt;
use crate::tts::SherpaTts;

pub struct SherpaFactory;

impl VendorFactory for SherpaFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(SherpaStt::new(config)))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(SherpaTts::new(config)))
    }

    fn create_language_id(
        &self,
        config: &LanguageIdConfig,
    ) -> SpeechResult<Option<Box<dyn LanguageIdProvider>>> {
        Ok(Some(Box::new(SherpaLanguageId::new(config))))
    }
}
