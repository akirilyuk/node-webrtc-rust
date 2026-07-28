//! Process-wide pool for Sherpa ONNX STT recognizers and TTS engines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel;
use sherpa_onnx::{OnlineRecognizer, OfflineTts};
use tokio::sync::Semaphore;

use crate::loader::{create_offline_tts, create_online_recognizer};
use crate::model_paths::resolve_stt_model_dir;
use crate::tts_model_paths::resolve_tts_model_dir_path;

static GLOBAL_POOL: OnceLock<Arc<SherpaModelPool>> = OnceLock::new();

/// Pool key for shared STT weights (canonical model directory).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SttPoolKey(PathBuf);

/// Pool key for shared TTS weights (canonical model directory).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TtsPoolKey(PathBuf);

/// RAII counter for Sherpa active sessions — decrements exactly once on drop.
///
/// Covers constructor/init failures, inference errors, panics, and normal end.
/// Double-drop / manual end is harmless (no underflow).
pub struct ActiveSessionGuard {
    counter: Arc<AtomicUsize>,
    armed: bool,
}

impl ActiveSessionGuard {
    fn acquire(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self {
            counter: Arc::clone(counter),
            armed: true,
        }
    }

    /// Explicit end (same as drop). Idempotent.
    pub fn end(mut self) {
        self.release_once();
    }

    fn release_once(&mut self) {
        if !self.armed {
            return;
        }
        self.armed = false;
        // Saturating decrement — never underflow on double-end races.
        let mut cur = self.counter.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return;
            }
            match self.counter.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(v) => cur = v,
            }
        }
    }
}

impl Drop for ActiveSessionGuard {
    fn drop(&mut self) {
        self.release_once();
    }
}

/// Shared streaming STT recognizer (one per model directory).
pub struct SharedSttRecognizer {
    recognizer: Mutex<OnlineRecognizer>,
    pub(crate) active_sessions: Arc<AtomicUsize>,
}

/// Shared offline TTS engine (one slot in a [`TtsEnginePool`]).
pub struct SharedTtsEngine {
    pub(crate) tts: Mutex<OfflineTts>,
    pub(crate) active_sessions: Arc<AtomicUsize>,
    tts_semaphore: Arc<Semaphore>,
}

/// Pool of offline TTS engines for one model directory (parallel synthesis up to pool size).
pub struct TtsEnginePool {
    engines: Vec<Arc<SharedTtsEngine>>,
    next: AtomicUsize,
}

impl TtsEnginePool {
    fn new(config: &TtsConfig, tts_semaphore: Arc<Semaphore>) -> SpeechResult<Self> {
        let slots = max_concurrent_tts();
        let mut engines = Vec::with_capacity(slots);
        for _ in 0..slots {
            let engine = create_offline_tts(config)?;
            engines.push(Arc::new(SharedTtsEngine::new(engine, Arc::clone(&tts_semaphore))));
        }
        Ok(Self {
            engines,
            next: AtomicUsize::new(0),
        })
    }

    pub fn acquire(&self) -> Arc<SharedTtsEngine> {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.engines.len();
        Arc::clone(&self.engines[index])
    }

    pub fn len(&self) -> usize {
        self.engines.len()
    }
}

/// Process-wide Sherpa model pool.
pub struct SherpaModelPool {
    stt: Mutex<HashMap<SttPoolKey, Arc<SharedSttRecognizer>>>,
    tts: Mutex<HashMap<TtsPoolKey, Arc<TtsEnginePool>>>,
    decode_semaphore: Arc<Semaphore>,
    tts_semaphore: Arc<Semaphore>,
}

impl SherpaModelPool {
    pub fn new() -> Self {
        Self {
            stt: Mutex::new(HashMap::new()),
            tts: Mutex::new(HashMap::new()),
            decode_semaphore: Arc::new(Semaphore::new(max_concurrent_decode())),
            tts_semaphore: Arc::new(Semaphore::new(max_concurrent_tts())),
        }
    }

    pub fn decode_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.decode_semaphore)
    }

    pub fn tts_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.tts_semaphore)
    }

    /// Returns the process-wide pool (lazy init).
    pub fn global() -> Arc<Self> {
        GLOBAL_POOL
            .get_or_init(|| Arc::new(Self::new()))
            .clone()
    }

    /// Acquire or create a shared STT recognizer for `config` (call from blocking context).
    pub fn get_or_create_stt(&self, config: &SttConfig) -> SpeechResult<Arc<SharedSttRecognizer>> {
        let key = stt_pool_key(config)?;
        let mut map = self
            .stt
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa STT pool lock poisoned".into()))?;
        if let Some(existing) = map.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let recognizer = create_online_recognizer(config)?;
        let shared = Arc::new(SharedSttRecognizer::new(recognizer));
        map.insert(key, Arc::clone(&shared));
        otel::set_sherpa_pool_entries((map.len() + self.tts.lock().expect("lock").len()) as i64);
        Ok(shared)
    }

    /// Acquire or create a shared TTS engine pool for `config` (call from blocking context).
    pub fn get_or_create_tts(&self, config: &TtsConfig) -> SpeechResult<Arc<TtsEnginePool>> {
        let key = tts_pool_key(config)?;
        let mut map = self
            .tts
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa TTS pool lock poisoned".into()))?;
        if let Some(existing) = map.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let pool = Arc::new(TtsEnginePool::new(
            config,
            Arc::clone(&self.tts_semaphore),
        )?);
        map.insert(key, Arc::clone(&pool));
        otel::set_sherpa_pool_entries((self.stt.lock().expect("lock").len() + map.len()) as i64);
        Ok(pool)
    }

    /// Number of distinct STT model directories loaded in the pool.
    pub fn stt_entry_count(&self) -> usize {
        self.stt.lock().expect("lock").len()
    }

    /// Number of distinct TTS model directories loaded in the pool.
    pub fn tts_entry_count(&self) -> usize {
        self.tts.lock().expect("lock").len()
    }

    /// Pointer identity of the shared STT entry for `config`, if loaded.
    pub fn shared_stt_ptr(&self, config: &SttConfig) -> Option<usize> {
        let key = stt_pool_key(config).ok()?;
        self.stt
            .lock()
            .ok()?
            .get(&key)
            .map(|entry| Arc::as_ptr(entry) as usize)
    }
}

impl SharedSttRecognizer {
    fn new(recognizer: OnlineRecognizer) -> Self {
        Self {
            recognizer: Mutex::new(recognizer),
            active_sessions: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Increment active sessions; pair with drop of the returned guard (RAII).
    pub fn track_session(&self) -> ActiveSessionGuard {
        ActiveSessionGuard::acquire(&self.active_sessions)
    }

    #[deprecated(note = "use track_session() RAII guard")]
    pub fn session_started(&self) {
        self.active_sessions.fetch_add(1, Ordering::SeqCst);
    }

    #[deprecated(note = "use track_session() RAII guard")]
    pub fn session_ended(&self) {
        let mut cur = self.active_sessions.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return;
            }
            match self.active_sessions.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(v) => cur = v,
            }
        }
    }

    pub fn active_sessions(&self) -> usize {
        self.active_sessions.load(Ordering::SeqCst)
    }

    pub fn create_stream(&self) -> sherpa_onnx::OnlineStream {
        let guard = self
            .recognizer
            .lock()
            .expect("sherpa recognizer lock poisoned");
        guard.create_stream()
    }

    pub fn with_recognizer<R>(&self, f: impl FnOnce(&OnlineRecognizer) -> R) -> R {
        let guard = self
            .recognizer
            .lock()
            .expect("sherpa recognizer lock poisoned");
        f(&guard)
    }
}

impl SharedTtsEngine {
    fn new(tts: OfflineTts, tts_semaphore: Arc<Semaphore>) -> Self {
        Self {
            tts: Mutex::new(tts),
            active_sessions: Arc::new(AtomicUsize::new(0)),
            tts_semaphore,
        }
    }

    pub fn tts_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.tts_semaphore)
    }

    /// Increment active sessions; pair with drop of the returned guard (RAII).
    pub fn track_session(&self) -> ActiveSessionGuard {
        ActiveSessionGuard::acquire(&self.active_sessions)
    }

    pub fn active_sessions(&self) -> usize {
        self.active_sessions.load(Ordering::SeqCst)
    }

    #[deprecated(note = "use track_session() RAII guard")]
    pub fn session_started(&self) {
        self.active_sessions.fetch_add(1, Ordering::SeqCst);
    }

    #[deprecated(note = "use track_session() RAII guard")]
    pub fn session_ended(&self) {
        let mut cur = self.active_sessions.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return;
            }
            match self.active_sessions.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(v) => cur = v,
            }
        }
    }
}

/// Canonical path for pool deduplication (best-effort `canonicalize`).
pub fn canonical_model_dir(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn stt_pool_key(config: &SttConfig) -> SpeechResult<SttPoolKey> {
    let model_dir = resolve_stt_model_dir(config)?;
    Ok(SttPoolKey(canonical_model_dir(&model_dir)))
}

pub fn tts_pool_key(config: &TtsConfig) -> SpeechResult<TtsPoolKey> {
    let model_dir = resolve_tts_model_dir_path(config)?;
    Ok(TtsPoolKey(canonical_model_dir(&model_dir)))
}

pub fn max_concurrent_decode() -> usize {
    parse_pool_limit_env("SHERPA_POOL_MAX_CONCURRENT_DECODE")
        .unwrap_or_else(default_max_concurrent_decode)
        .max(1)
}

pub fn max_concurrent_tts() -> usize {
    parse_pool_limit_env("SHERPA_POOL_MAX_CONCURRENT_TTS")
        .unwrap_or(2)
        .max(1)
}

fn default_max_concurrent_decode() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .max(1)
}

fn parse_pool_limit_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|&limit| limit > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    #[test]
    fn canonical_model_dir_nonexistent_preserves_path() {
        let path = unique_temp_dir("sherpa-canonical-missing");
        assert_eq!(canonical_model_dir(&path), path);
    }

    #[test]
    fn stt_pool_keys_match_for_same_model_path() {
        let dir = unique_temp_dir("sherpa-stt-key");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let config_a = SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::LocalSherpa,
            model: None,
            model_path: Some(dir.display().to_string()),
            language: Some("en".into()),
            api_key: None,
        };
        let config_b = SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::LocalSherpa,
            model: None,
            model_path: Some(dir.display().to_string()),
            language: Some("en".into()),
            api_key: None,
        };
        assert_eq!(stt_pool_key(&config_a).unwrap(), stt_pool_key(&config_b).unwrap());
    }

    #[test]
    fn stt_pool_keys_differ_for_different_dirs() {
        let dir_a = unique_temp_dir("sherpa-stt-a");
        let dir_b = unique_temp_dir("sherpa-stt-b");
        std::fs::create_dir_all(&dir_a).expect("mkdir a");
        std::fs::create_dir_all(&dir_b).expect("mkdir b");
        let key_a = stt_pool_key(&SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::LocalSherpa,
            model: None,
            model_path: Some(dir_a.display().to_string()),
            language: None,
            api_key: None,
        })
        .unwrap();
        let key_b = stt_pool_key(&SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::LocalSherpa,
            model: None,
            model_path: Some(dir_b.display().to_string()),
            language: None,
            api_key: None,
        })
        .unwrap();
        assert_ne!(key_a, key_b);
    }

    #[test]
    fn tts_pool_keys_match_for_same_model_path_different_speaker() {
        let dir = unique_temp_dir("sherpa-tts-key");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let config_a = TtsConfig {
            provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(dir.display().to_string()),
            voice: Some("0".into()),
            api_key: None,
        };
        let config_b = TtsConfig {
            provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(dir.display().to_string()),
            voice: Some("1".into()),
            api_key: None,
        };
        assert_eq!(tts_pool_key(&config_a).unwrap(), tts_pool_key(&config_b).unwrap());
    }

    #[test]
    fn max_concurrent_decode_defaults_to_at_least_one() {
        assert!(default_max_concurrent_decode() >= 1);
    }

    #[test]
    fn parse_pool_limit_env_rejects_zero() {
        let key = format!("SHERPA_POOL_TEST_ZERO_{}", std::process::id());
        // SAFETY: test runs sequentially for env mutation.
        unsafe { std::env::set_var(&key, "0") };
        assert!(parse_pool_limit_env(&key).is_none());
        unsafe { std::env::remove_var(&key) };
    }

    #[test]
    fn global_pool_returns_same_arc() {
        let a = SherpaModelPool::global();
        let b = SherpaModelPool::global();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn active_session_guard_returns_to_baseline_on_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        {
            let _g = ActiveSessionGuard::acquire(&counter);
            assert_eq!(counter.load(Ordering::SeqCst), 1);
            {
                let _g2 = ActiveSessionGuard::acquire(&counter);
                assert_eq!(counter.load(Ordering::SeqCst), 2);
            }
            assert_eq!(counter.load(Ordering::SeqCst), 1);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn active_session_guard_end_is_idempotent_with_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let g = ActiveSessionGuard::acquire(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        g.end();
        // Drop of moved value already ran inside end(); counter stays at baseline.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn active_session_guard_double_end_does_not_underflow() {
        let counter = Arc::new(AtomicUsize::new(0));
        let g = ActiveSessionGuard::acquire(&counter);
        // Simulate a buggy second decrement path against the same counter.
        let mut cur = counter.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                break;
            }
            match counter.compare_exchange_weak(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
        // Guard drop must not underflow below zero.
        drop(g);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn shared_stt_track_session_baseline() {
        // Use a throwaway recognizer-less counter path via ActiveSessionGuard only —
        // SharedSttRecognizer::new needs a real OnlineRecognizer.
        let counter = Arc::new(AtomicUsize::new(7));
        let baseline = counter.load(Ordering::SeqCst);
        {
            let g = ActiveSessionGuard::acquire(&counter);
            assert_eq!(counter.load(Ordering::SeqCst), baseline + 1);
            // Early "error" path: drop without explicit end.
            drop(g);
        }
        assert_eq!(counter.load(Ordering::SeqCst), baseline);
    }

    /// Mirrors Sherpa TTS: guard lifetime is inside `spawn_blocking`, so aborting
    /// the parent await must not drop the active count while blocking work runs.
    #[tokio::test]
    async fn active_session_guard_inside_spawn_blocking_survives_parent_abort() {
        use std::sync::Barrier;

        let counter = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));

        let counter_blocking = Arc::clone(&counter);
        let entered_blocking = Arc::clone(&entered);
        let release_blocking = Arc::clone(&release);
        let blocking = tokio::task::spawn_blocking(move || {
            let _active = ActiveSessionGuard::acquire(&counter_blocking);
            entered_blocking.wait();
            release_blocking.wait();
            // Guard drops here — after blocking work finishes.
        });

        // Wait until the guard is held inside the blocking thread.
        entered.wait();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Abort a parent task that was merely awaiting the JoinHandle — equivalent
        // to VoiceAgent aborting its Tokio worker while OfflineTts still runs.
        let waiter = tokio::spawn(async move {
            let _ = blocking.await;
        });
        waiter.abort();
        let _ = waiter.await;

        // Guard must still be alive: aborting the waiter does not cancel spawn_blocking.
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "outer abort must not drop ActiveSessionGuard held inside spawn_blocking"
        );

        release.wait();
        // Allow the blocking thread to finish and drop the guard.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

}
