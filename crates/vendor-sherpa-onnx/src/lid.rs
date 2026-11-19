use std::sync::Arc;
use std::thread;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use node_webrtc_rust_speech::config::{decide_language_among_allowed, LanguageIdConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32;
use node_webrtc_rust_speech::pipeline::{LanguageIdProvider, LanguageIdResult};
use tokio::sync::oneshot;

use crate::loader::voice_debug;
use crate::native_calls::NativeCallGuard;
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
        if allowlist_set(config) {
            // A scorer that fails to load or score never makes LID worse than sherpa's own
            // argmax: log one line and fall back to `identify_samples` (the allowlist then
            // filters that code as before).
            match decide_with_scorer(config, &shared, &samples) {
                Ok(Some(result)) => {
                    voice_debug(format!(
                        "LID raw={} decided={} gap={:.2} samples={} total_ms={}",
                        result.raw_language.as_deref().unwrap_or(""),
                        result.language,
                        result.gap.unwrap_or(0.0),
                        samples.len(),
                        started.elapsed().as_millis()
                    ));
                    return Ok(Some(result));
                }
                Ok(None) => {}
                Err(err) => {
                    voice_debug(format!(
                        "LID allowed-set scorer failed, using sherpa identify: {err}"
                    ));
                }
            }
        }
        let lang = shared.identify_samples(&samples)?;
        voice_debug(format!(
            "LID identify: samples={} model_wait_ms={} (pool_entries_before={}) total_ms={}",
            samples.len(),
            model_wait_ms,
            loaded_before,
            started.elapsed().as_millis()
        ));
        Ok(lang.map(LanguageIdResult::new))
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

/// Score every Whisper language and decide among the allowlist. `Err` when the scorer fails to
/// load or to score; the caller falls back to sherpa's argmax.
fn decide_with_scorer(
    config: &LanguageIdConfig,
    shared: &SharedLidRecognizer,
    samples: &[f32],
) -> SpeechResult<Option<LanguageIdResult>> {
    let scorer = shared.scorer(config)?;
    let scores = {
        let _native = NativeCallGuard::enter();
        scorer.language_scores(samples)?
    };
    Ok(decide_language_among_allowed(
        config,
        &scores.codes,
        &scores.logits,
    ))
}

/// True when `config` names at least one allowlisted language (decide among them).
pub(crate) fn allowlist_set(config: &LanguageIdConfig) -> bool {
    config
        .allowlist
        .as_ref()
        .is_some_and(|list| list.iter().any(|c| !c.trim().is_empty()))
}

impl SharedLidRecognizer {
    /// Whisper language scorer on the same model files (loaded once, shared).
    pub(crate) fn scorer(
        &self,
        config: &LanguageIdConfig,
    ) -> SpeechResult<Arc<crate::whisper_lang::WhisperLanguageScorer>> {
        self.scorer
            .get_or_init(|| {
                crate::whisper_lang::WhisperLanguageScorer::load_for_config(config)
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            })
            .clone()
            .map_err(|message| SpeechError::Vendor {
                vendor: "local-sherpa".into(),
                message,
            })
    }

    pub(crate) fn identify_samples(&self, samples: &[f32]) -> SpeechResult<Option<String>> {
        let _native = NativeCallGuard::enter();
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

#[cfg(test)]
mod tests {
    //! Scorer failure falls back to sherpa. `#[ignore]`: needs `SHERPA_LID_MODEL_PATH` (Whisper
    //! tiny); CI runs them in `scripts/ci/run-sherpa-example-ci.sh`.
    use super::*;

    fn config(allowlist: Option<Vec<&str>>) -> LanguageIdConfig {
        LanguageIdConfig {
            enabled: None,
            model_path: Some(
                std::env::var("SHERPA_LID_MODEL_PATH").expect("set SHERPA_LID_MODEL_PATH"),
            ),
            allowlist: allowlist.map(|l| l.into_iter().map(String::from).collect()),
            allowlist_max_gap: None,
            allowlist_min_margin: None,
            report_out_of_set: None,
            min_speech_ms: None,
            continuous: None,
            timing: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: None,
        }
    }

    /// Real-path Spanish clip whose unrestricted Whisper-tiny argmax is `ja` (scorer decides `es`).
    fn fixture_pcm() -> Bytes {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/lid-allowed-set/es-miss-1787-a150-utt2.wav");
        let bytes = std::fs::read(path).expect("fixture");
        assert_eq!(&bytes[0..4], b"RIFF");
        Bytes::copy_from_slice(&bytes[44..])
    }

    /// A pool whose LID entry for `config` has a scorer that failed to load.
    fn pool_with_unloadable_scorer(config: &LanguageIdConfig) -> SherpaModelPool {
        let pool = SherpaModelPool::new();
        let shared = pool.get_or_create_lid(config).expect("sherpa lid");
        assert!(
            shared
                .scorer
                .set(Err("forced: scorer model unloadable".into()))
                .is_ok(),
            "scorer not loaded yet"
        );
        pool
    }

    #[test]
    #[ignore = "needs SHERPA_LID_MODEL_PATH (Whisper tiny)"]
    fn scorer_failure_returns_sherpa_identify_code() {
        let plain = SherpaModelPool::new();
        let sherpa = SherpaLanguageId::identify_blocking(&config(None), &plain, fixture_pcm())
            .expect("sherpa identify")
            .expect("sherpa result");

        let restricted = config(Some(vec!["en", "es"]));
        let pool = pool_with_unloadable_scorer(&restricted);
        let result = SherpaLanguageId::identify_blocking(&restricted, &pool, fixture_pcm())
            .expect("scorer failure must not fail identify")
            .expect("result");
        assert_eq!(result.language, sherpa.language);
        assert_eq!(result.raw_language, None, "{result:?}");
        assert_eq!(result.gap, None, "{result:?}");
    }

    #[test]
    #[ignore = "needs SHERPA_LID_MODEL_PATH (Whisper tiny)"]
    fn preload_succeeds_when_only_the_scorer_fails() {
        let restricted = config(Some(vec!["en", "es"]));
        let pool = pool_with_unloadable_scorer(&restricted);
        pool.preload_lid(&restricted)
            .expect("preload must not fail on a scorer-only failure");
    }
}
