//! Vendor registry for STT/TTS provider construction.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use crate::error::{SpeechError, SpeechResult};
use crate::pipeline::{SttProvider, TtsProvider, VendorFactory};

/// Registry mapping vendor ids to factory implementations.
pub struct VendorRegistry {
    stt: HashMap<SttVendor, Arc<dyn VendorFactory>>,
    tts: HashMap<TtsVendor, Arc<dyn VendorFactory>>,
}

impl VendorRegistry {
    pub fn new() -> Self {
        Self {
            stt: HashMap::new(),
            tts: HashMap::new(),
        }
    }

    pub fn register_stt(&mut self, vendor: SttVendor, factory: Arc<dyn VendorFactory>) {
        self.stt.insert(vendor, factory);
    }

    pub fn register_tts(&mut self, vendor: TtsVendor, factory: Arc<dyn VendorFactory>) {
        self.tts.insert(vendor, factory);
    }

    pub fn create_stt(&self, config: &SttConfig) -> SpeechResult<Box<dyn SttProvider>> {
        let factory = self.stt.get(&config.provider).ok_or_else(|| {
            SpeechError::Config(format!("unsupported STT vendor: {:?}", config.provider))
        })?;
        factory.create_stt(config)
    }

    pub fn create_tts(&self, config: &TtsConfig) -> SpeechResult<Box<dyn TtsProvider>> {
        let factory = self.tts.get(&config.provider).ok_or_else(|| {
            SpeechError::Config(format!("unsupported TTS vendor: {:?}", config.provider))
        })?;
        factory.create_tts(config)
    }
}

impl Default for VendorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
