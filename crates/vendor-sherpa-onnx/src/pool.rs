//! Process-wide pool for Sherpa ONNX STT recognizers and TTS engines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use node_webrtc_rust_speech::config::{SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
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

/// Shared streaming STT recognizer (one per model directory).
pub struct SharedSttRecognizer {
    recognizer: Mutex<OnlineRecognizer>,
    pub(crate) active_sessions: AtomicUsize,
}

/// Shared offline TTS engine (one per model directory).
pub struct SharedTtsEngine {
    pub(crate) tts: Mutex<OfflineTts>,
    synthesis_mutex: Mutex<()>,
    pub(crate) active_sessions: AtomicUsize,
    tts_semaphore: Arc<Semaphore>,
}

/// Process-wide Sherpa model pool.
pub struct SherpaModelPool {
    stt: Mutex<HashMap<SttPoolKey, Arc<SharedSttRecognizer>>>,
    tts: Mutex<HashMap<TtsPoolKey, Arc<SharedTtsEngine>>>,
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
        Ok(shared)
    }

    /// Acquire or create a shared TTS engine for `config` (call from blocking context).
    pub fn get_or_create_tts(&self, config: &TtsConfig) -> SpeechResult<Arc<SharedTtsEngine>> {
        let key = tts_pool_key(config)?;
        let mut map = self
            .tts
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa TTS pool lock poisoned".into()))?;
        if let Some(existing) = map.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let engine = create_offline_tts(config)?;
        let shared = Arc::new(SharedTtsEngine::new(
            engine,
            Arc::clone(&self.tts_semaphore),
        ));
        map.insert(key, Arc::clone(&shared));
        Ok(shared)
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
            active_sessions: AtomicUsize::new(0),
        }
    }

    pub fn session_started(&self) {
        self.active_sessions.fetch_add(1, Ordering::SeqCst);
    }

    pub fn session_ended(&self) {
        self.active_sessions.fetch_sub(1, Ordering::SeqCst);
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
            synthesis_mutex: Mutex::new(()),
            active_sessions: AtomicUsize::new(0),
            tts_semaphore,
        }
    }

    pub fn tts_semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.tts_semaphore)
    }

    pub fn session_started(&self) {
        self.active_sessions.fetch_add(1, Ordering::SeqCst);
    }

    pub fn session_ended(&self) {
        self.active_sessions.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn synthesis_mutex(&self) -> &Mutex<()> {
        &self.synthesis_mutex
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

}
