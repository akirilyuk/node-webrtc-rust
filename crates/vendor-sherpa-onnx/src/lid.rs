use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::LanguageIdConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::{LanguageIdProvider, LanguageIdResult};

use crate::pool::{SharedLidRecognizer, SherpaModelPool};

pub(crate) const SAMPLE_RATE: i32 = 16_000;

pub struct SherpaLanguageId {
    config: LanguageIdConfig,
    pool: Arc<SherpaModelPool>,
}

impl SherpaLanguageId {
    pub fn new(config: &LanguageIdConfig) -> Self {
        Self {
            config: config.clone(),
            pool: SherpaModelPool::global(),
        }
    }

    fn identify_blocking(
        config: &LanguageIdConfig,
        pool: &SherpaModelPool,
        pcm: Bytes,
    ) -> SpeechResult<Option<LanguageIdResult>> {
        if pcm.is_empty() {
            return Ok(None);
        }
        let shared = pool.get_or_create_lid(config)?;
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        let lang = shared.identify_samples(&samples)?;
        Ok(lang.map(|language| LanguageIdResult { language }))
    }
}

#[async_trait]
impl LanguageIdProvider for SherpaLanguageId {
    async fn identify(
        &self,
        pcm: Bytes,
        sample_rate: u32,
    ) -> SpeechResult<Option<LanguageIdResult>> {
        if sample_rate != SAMPLE_RATE as u32 {
            return Err(SpeechError::Config(format!(
                "Sherpa LID expects {} Hz mono PCM, got {} Hz",
                SAMPLE_RATE, sample_rate
            )));
        }
        let config = self.config.clone();
        let pool = Arc::clone(&self.pool);
        tokio::task::spawn_blocking(move || Self::identify_blocking(&config, &pool, pcm))
            .await
            .map_err(|err| SpeechError::Internal(err.to_string()))?
    }
}

impl SharedLidRecognizer {
    pub(crate) fn identify_samples(&self, samples: &[f32]) -> SpeechResult<Option<String>> {
        let guard = self
            .identifier
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa LID lock poisoned".into()))?;
        let stream = guard.create_stream();
        stream.accept_waveform(SAMPLE_RATE, samples);
        let result = guard.compute(&stream).ok_or_else(|| SpeechError::Vendor {
            vendor: "local-sherpa".into(),
            message: "SpokenLanguageIdentification::compute returned no result".into(),
        })?;
        let lang = result.lang.trim().to_ascii_lowercase();
        if lang.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lang))
        }
    }
}
