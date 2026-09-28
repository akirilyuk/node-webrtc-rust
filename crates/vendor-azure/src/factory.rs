use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};

use crate::stt::AzureStt;
use crate::tts::AzureTts;

pub struct AzureFactory;

impl VendorFactory for AzureFactory {
    fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        Ok(Box::new(AzureStt::new(config)?))
    }

    fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        Ok(Box::new(AzureTts::new(config)?))
    }
}

pub(crate) fn speech_key_from(config_key: &Option<String>) -> SpeechResult<String> {
    if let Some(key) = config_key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }
    for env in ["AZURE_SPEECH_KEY", "SPEECH_KEY"] {
        if let Ok(key) = std::env::var(env) {
            if !key.is_empty() {
                return Ok(key);
            }
        }
    }
    Err(SpeechError::Config(
        "missing Azure Speech key: set config.apiKey, AZURE_SPEECH_KEY, or SPEECH_KEY".into(),
    ))
}
