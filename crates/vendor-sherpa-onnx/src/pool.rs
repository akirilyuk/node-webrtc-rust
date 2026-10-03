//! Process-wide pool for Sherpa ONNX STT recognizers and TTS engines.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

use node_webrtc_rust_speech::config::{LanguageIdConfig, SttConfig, TtsConfig};
use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use node_webrtc_rust_speech::otel;
use sherpa_onnx::{OfflineTts, OnlineRecognizer, SpokenLanguageIdentification};
use tokio::sync::Semaphore;

use crate::lid_model_paths::lid_pool_key;
use crate::loader::{
    create_offline_tts, create_online_recognizer, create_spoken_language_identification,
    voice_debug,
};
use crate::model_paths::resolve_stt_model_dir;
use crate::tts_model_paths::resolve_tts_model_dir_path;

static GLOBAL_POOL: OnceLock<Arc<SherpaModelPool>> = OnceLock::new();

extern "C" {
    fn atexit(func: extern "C" fn()) -> i32;
}

/// Join TTS and LID preload threads before ONNX static destructors run.
///
/// `OnceLock` statics are not dropped on process exit. A detached preload
/// thread still inside `create_offline_tts` then races Sherpa teardown
/// (SIGSEGV after otherwise-green ignored TTS tests). `atexit` is LIFO, and
/// this handler is registered after the first engine exists, so it runs
/// before ONNX's own exit handlers.
extern "C" fn join_tts_preloads_at_exit() {
    let Some(pool) = GLOBAL_POOL.get() else {
        return;
    };
    pool.join_tts_preloads();
    pool.join_lid_preloads();
}

fn register_tts_preload_exit_join() {
    static REGISTERED: Once = Once::new();
    REGISTERED.call_once(|| {
        // SAFETY: process-global C `atexit`. The handler only joins threads
        // this pool already spawned.
        let rc = unsafe { atexit(join_tts_preloads_at_exit) };
        if rc != 0 {
            eprintln!("sherpa tts preload exit join registration failed");
        }
    });
}

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
///
/// The first engine is loaded before `new` returns so the process can listen.
/// Further engines load on a background thread; until they finish, acquire
/// shares the engines that are already up.
///
/// The preload [`JoinHandle`] is joined in [`Drop`] before the engines are
/// dropped. Discarding it detaches the thread; ONNX teardown then races the
/// still-running `create_offline_tts` and SIGSEGV on process exit.
pub struct TtsEnginePool {
    engines: Arc<Mutex<Vec<Arc<SharedTtsEngine>>>>,
    next: AtomicUsize,
    /// `None` when only one slot is configured or the thread failed to spawn.
    preload: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl TtsEnginePool {
    fn new(config: &TtsConfig, tts_semaphore: Arc<Semaphore>) -> SpeechResult<Self> {
        let slots = max_concurrent_tts().max(1);
        let first = create_offline_tts(config)?;
        let engines = Arc::new(Mutex::new(vec![Arc::new(SharedTtsEngine::new(
            first,
            Arc::clone(&tts_semaphore),
        ))]));
        let preload = if slots > 1 {
            register_tts_preload_exit_join();
            let config = config.clone();
            let pending = Arc::clone(&engines);
            let semaphore = Arc::clone(&tts_semaphore);
            std::thread::Builder::new()
                .name("sherpa-tts-preload".into())
                .spawn(move || {
                    for _ in 1..slots {
                        match create_offline_tts(&config) {
                            Ok(engine) => {
                                let Ok(mut guard) = pending.lock() else {
                                    return;
                                };
                                guard.push(Arc::new(SharedTtsEngine::new(
                                    engine,
                                    Arc::clone(&semaphore),
                                )));
                            }
                            Err(error) => {
                                eprintln!("sherpa tts extra engine load failed: {error}");
                                return;
                            }
                        }
                    }
                })
                .ok()
        } else {
            None
        };
        Ok(Self {
            engines,
            next: AtomicUsize::new(0),
            preload: Mutex::new(preload),
        })
    }

    /// Wait for extra engines before `OfflineTts` drops. Does not lock `engines`
    /// (the preload thread locks that mutex only to push).
    fn join_preload(&self) {
        let mut slot = match self.preload.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(handle) = slot.take() {
            let _ = handle.join();
        }
    }

    pub fn acquire(&self) -> Arc<SharedTtsEngine> {
        let engines = self.engines.lock().expect("tts engine pool lock");
        let index = self.next.fetch_add(1, Ordering::Relaxed) % engines.len();
        Arc::clone(&engines[index])
    }

    pub fn len(&self) -> usize {
        self.engines
            .lock()
            .map(|engines| engines.len())
            .unwrap_or(0)
    }
}

impl Drop for TtsEnginePool {
    fn drop(&mut self) {
        self.join_preload();
    }
}

/// Pool key for shared LID weights (canonical model directory).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LidPoolKey(PathBuf);

/// Shared spoken-language identifier (one per model directory, process-wide).
///
/// Thread-safety: `SpokenLanguageIdentification` is not documented as safe for concurrent
/// `compute` on one instance, so the model sits behind a [`Mutex`]. Each inference locks it,
/// creates a fresh per-call stream, computes, and drops the stream; concurrent sessions
/// therefore serialise on inference (tiny Whisper, a few hundred ms) but never reload the
/// 250 MB+ weights. The pool map lock is held while a model loads, so concurrent first callers
/// for the same directory wait for one load instead of loading twice (single flight).
pub struct SharedLidRecognizer {
    pub(crate) identifier: Mutex<SpokenLanguageIdentification>,
    pub(crate) active_sessions: Arc<AtomicUsize>,
}

/// Process-wide Sherpa model pool.
pub struct SherpaModelPool {
    stt: Mutex<HashMap<SttPoolKey, Arc<SharedSttRecognizer>>>,
    tts: Mutex<HashMap<TtsPoolKey, Arc<TtsEnginePool>>>,
    lid: Mutex<HashMap<LidPoolKey, Arc<SharedLidRecognizer>>>,
    /// Model directories whose LID model is resident. Written (briefly) right after the model is
    /// inserted into `lid`, and never held across a load, so callers on a session's setup path
    /// can ask "is it loaded?" without queueing behind the `lid` map lock a loading thread holds.
    /// Lock order: `lid` then `lid_loaded` (nothing takes them the other way round).
    lid_loaded: Mutex<HashSet<LidPoolKey>>,
    /// Model directories with a background LID preload thread currently running.
    lid_preloading: Mutex<HashSet<LidPoolKey>>,
    lid_preload_handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
    decode_semaphore: Arc<Semaphore>,
    tts_semaphore: Arc<Semaphore>,
}

impl SherpaModelPool {
    pub fn new() -> Self {
        Self {
            stt: Mutex::new(HashMap::new()),
            tts: Mutex::new(HashMap::new()),
            lid: Mutex::new(HashMap::new()),
            lid_loaded: Mutex::new(HashSet::new()),
            lid_preloading: Mutex::new(HashSet::new()),
            lid_preload_handles: Mutex::new(Vec::new()),
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

    /// Join every TTS preload thread. Safe to call more than once.
    fn join_tts_preloads(&self) {
        let Ok(map) = self.tts.lock() else {
            return;
        };
        for pool in map.values() {
            pool.join_preload();
        }
    }

    /// Publish the total pooled-model gauge.
    ///
    /// Takes each map lock on its own, never while another map lock (or a model load) is held.
    /// Nesting them (stt -> lid in one path, lid -> stt in another) deadlocked an STT load
    /// against a concurrent LID load: callers must drop their own map guard first.
    fn publish_entry_gauge(&self) {
        let stt = self.stt.lock().map(|m| m.len()).unwrap_or(0);
        let tts = self.tts.lock().map(|m| m.len()).unwrap_or(0);
        let lid = self.lid.lock().map(|m| m.len()).unwrap_or(0);
        otel::set_sherpa_pool_entries((stt + tts + lid) as i64);
    }

    /// Join every LID preload thread (blocks until background loads finish). Safe to call
    /// more than once; also runs at process exit.
    pub fn join_lid_preloads(&self) {
        let handles = match self.lid_preload_handles.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => return,
        };
        for handle in handles {
            let _ = handle.join();
        }
    }

    /// Returns the process-wide pool (lazy init).
    pub fn global() -> Arc<Self> {
        GLOBAL_POOL.get_or_init(|| Arc::new(Self::new())).clone()
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
        drop(map);
        self.publish_entry_gauge();
        Ok(shared)
    }

    /// Acquire or create a shared LID model for `config` (call from blocking context).
    pub fn get_or_create_lid(
        &self,
        config: &LanguageIdConfig,
    ) -> SpeechResult<Arc<SharedLidRecognizer>> {
        let key = LidPoolKey(lid_pool_key(config)?);
        let mut map = self
            .lid
            .lock()
            .map_err(|_| SpeechError::Internal("sherpa LID pool lock poisoned".into()))?;
        if let Some(existing) = map.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let identifier = create_spoken_language_identification(config)?;
        let shared = Arc::new(SharedLidRecognizer::new(identifier));
        map.insert(key.clone(), Arc::clone(&shared));
        if let Ok(mut loaded) = self.lid_loaded.lock() {
            loaded.insert(key);
        }
        drop(map);
        self.publish_entry_gauge();
        Ok(shared)
    }

    /// Load (or find) the shared LID model for `config`, blocking until it is resident.
    ///
    /// Call from a blocking context at process boot so the first utterance never pays the
    /// load. Idempotent: a second call for the same model directory is a pool lookup.
    pub fn preload_lid(&self, config: &LanguageIdConfig) -> SpeechResult<()> {
        self.get_or_create_lid(config).map(|_| ())
    }

    /// Start loading the shared LID model on a background thread and return immediately.
    ///
    /// No-op when the model is already loaded or a preload for the same directory is running.
    /// Never blocks on a model load in progress (see `lid_loaded`). Errors are only logged (`VOICE_DEBUG`): the first identify retries and reports them.
    pub fn spawn_lid_preload(self: &Arc<Self>, config: &LanguageIdConfig) {
        let Ok(key) = lid_pool_key(config).map(LidPoolKey) else {
            return;
        };
        // Runs on the caller's thread (a session being set up: Node main thread or a tokio
        // worker). Never touch the `lid` map lock here: a loading model holds it for the whole
        // load (hundreds of ms warm, seconds cold), which would stall every session this thread
        // serves. `lid_loaded` is only ever held for a set insert/lookup.
        if self
            .lid_loaded
            .lock()
            .map(|loaded| loaded.contains(&key))
            .unwrap_or(true)
        {
            return;
        }
        {
            let Ok(mut running) = self.lid_preloading.lock() else {
                return;
            };
            if !running.insert(key.clone()) {
                return;
            }
        }
        register_tts_preload_exit_join();
        let pool = Arc::clone(self);
        let config = config.clone();
        let spawned = std::thread::Builder::new()
            .name("sherpa-lid-preload".into())
            .spawn(move || {
                if let Err(error) = pool.preload_lid(&config) {
                    voice_debug(format!("LID preload failed: {error}"));
                }
                if let Ok(mut running) = pool.lid_preloading.lock() {
                    running.remove(&key);
                }
            });
        match spawned {
            Ok(handle) => {
                if let Ok(mut handles) = self.lid_preload_handles.lock() {
                    handles.retain(|h| !h.is_finished());
                    handles.push(handle);
                }
            }
            Err(error) => {
                voice_debug(format!("LID preload thread spawn failed: {error}"));
            }
        }
    }

    /// Pointer identity of the shared LID entry for `config`, if loaded.
    pub fn shared_lid_ptr(&self, config: &LanguageIdConfig) -> Option<usize> {
        let key = LidPoolKey(lid_pool_key(config).ok()?);
        self.lid
            .lock()
            .ok()?
            .get(&key)
            .map(|entry| Arc::as_ptr(entry) as usize)
    }

    /// Number of distinct LID model directories loaded in the pool.
    pub fn lid_entry_count(&self) -> usize {
        self.lid.lock().expect("lock").len()
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
        let pool = Arc::new(TtsEnginePool::new(config, Arc::clone(&self.tts_semaphore))?);
        map.insert(key, Arc::clone(&pool));
        drop(map);
        self.publish_entry_gauge();
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

impl SharedLidRecognizer {
    fn new(identifier: SpokenLanguageIdentification) -> Self {
        Self {
            identifier: Mutex::new(identifier),
            active_sessions: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn track_session(&self) -> ActiveSessionGuard {
        ActiveSessionGuard::acquire(&self.active_sessions)
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
    fn spawn_lid_preload_does_not_wait_for_a_model_that_is_loading() {
        use std::sync::mpsc;
        use std::time::Duration;

        let pool = Arc::new(SherpaModelPool::new());
        let config = LanguageIdConfig {
            enabled: Some(true),
            model_path: Some("/nonexistent/nwr-lid-preload-nonblocking".into()),
            allowlist: None,
            min_speech_ms: None,
            continuous: None,
            lid_max_clip_ms: None,
            lid_gate_max_wait_ms: None,
            tts_exclusion: None,
        };

        // `get_or_create_lid` holds the `lid` map lock for the whole model load. Hold it here to
        // park a "load in progress" deterministically, then construct a second session's
        // provider: it must return without waiting for that load.
        let loading = pool.lid.lock().expect("lid map lock");
        let (done_tx, done_rx) = mpsc::channel();
        let setup_pool = Arc::clone(&pool);
        let setup_config = config.clone();
        let setup = std::thread::spawn(move || {
            setup_pool.spawn_lid_preload(&setup_config);
            let _ = done_tx.send(());
        });
        let returned = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
        drop(loading);
        setup.join().expect("setup thread");
        pool.join_lid_preloads();
        assert!(
            returned,
            "spawn_lid_preload blocked on the LID pool lock held by a model load"
        );
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
            endpoint: None,
        };
        let config_b = SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::LocalSherpa,
            model: None,
            model_path: Some(dir.display().to_string()),
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        };
        assert_eq!(
            stt_pool_key(&config_a).unwrap(),
            stt_pool_key(&config_b).unwrap()
        );
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
            endpoint: None,
        })
        .unwrap();
        let key_b = stt_pool_key(&SttConfig {
            provider: node_webrtc_rust_speech::config::SttVendor::LocalSherpa,
            model: None,
            model_path: Some(dir_b.display().to_string()),
            language: None,
            api_key: None,
            endpoint: None,
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
            endpoint: None,
        };
        let config_b = TtsConfig {
            provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(dir.display().to_string()),
            voice: Some("1".into()),
            api_key: None,
            endpoint: None,
        };
        assert_eq!(
            tts_pool_key(&config_a).unwrap(),
            tts_pool_key(&config_b).unwrap()
        );
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
