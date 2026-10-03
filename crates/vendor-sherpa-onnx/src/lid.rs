use std::sync::Arc;
use std::thread;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::LanguageIdConfig;
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::{LanguageIdProvider, LanguageIdResult};
use tokio::sync::oneshot;

use crate::loader::voice_debug;
use crate::pool::{SharedLidRecognizer, SherpaModelPool};

pub(crate) const SAMPLE_RATE: i32 = 16_000;

pub struct SherpaLanguageId {
    config: LanguageIdConfig,
    pool: Arc<SherpaModelPool>,
}

/// Load the process-wide LID model for `config` now (blocking). Call from a blocking thread at
/// boot (e.g. runner start) so the first utterance of the first session never pays the load.
/// Idempotent and shared: later sessions with the same `modelPath` reuse the loaded model.
pub fn preload_language_id(config: &LanguageIdConfig) -> SpeechResult<()> {
    SherpaModelPool::global().preload_lid(config)
}

/// Async wrapper around [`preload_language_id`] (runs on tokio's blocking pool).
pub async fn preload_language_id_async(config: LanguageIdConfig) -> SpeechResult<()> {
    tokio::task::spawn_blocking(move || preload_language_id(&config))
        .await
        .map_err(|err| SpeechError::Internal(err.to_string()))?
}

impl SherpaLanguageId {
    /// Creates the provider and starts loading the shared LID model in the background, so the
    /// load overlaps session setup instead of landing on the first identify.
    pub fn new(config: &LanguageIdConfig) -> Self {
        let pool = SherpaModelPool::global();
        pool.spawn_lid_preload(config);
        Self {
            config: config.clone(),
            pool,
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
        let started = Instant::now();
        let loaded_before = pool.lid_entry_count();
        let shared = pool.get_or_create_lid(config)?;
        let model_wait_ms = started.elapsed().as_millis();
        let samples = mono_s16le_bytes_to_f32(pcm.as_ref());
        let lang = shared.identify_samples(&samples)?;
        voice_debug(format!(
            "LID identify: samples={} model_wait_ms={} (pool_entries_before={}) total_ms={}",
            samples.len(),
            model_wait_ms,
            loaded_before,
            started.elapsed().as_millis()
        ));
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
        // Dedicated OS thread — not tokio's blocking pool shared with Zipformer STT and Piper TTS.
        let (tx, rx) = oneshot::channel();
        thread::Builder::new()
            .name("sherpa-lid".into())
            .spawn(move || {
                let result = Self::identify_blocking(&config, &pool, pcm);
                let _ = tx.send(result);
            })
            .map_err(|err| SpeechError::Internal(err.to_string()))?;
        rx.await
            .map_err(|_| SpeechError::Internal("sherpa LID thread dropped".into()))?
    }
}

impl SharedLidRecognizer {
    pub(crate) fn identify_samples(&self, samples: &[f32]) -> SpeechResult<Option<String>> {
        let lock_started = Instant::now();
        let guard = self
            .identifier
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa LID lock poisoned".into()))?;
        let lock_wait_ms = lock_started.elapsed().as_millis();
        let compute_started = Instant::now();
        let stream = guard.create_stream();
        stream.accept_waveform(SAMPLE_RATE, samples);
        let computed = guard.compute(&stream);
        voice_debug(format!(
            "LID compute: samples={} lock_wait_ms={} compute_ms={}",
            samples.len(),
            lock_wait_ms,
            compute_started.elapsed().as_millis()
        ));
        let result = computed.ok_or_else(|| SpeechError::Vendor {
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
