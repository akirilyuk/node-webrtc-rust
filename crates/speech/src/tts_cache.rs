//! Runner-side TTS phrase cache.
//!
//! [`CachingTtsProvider`] wraps any [`TtsProvider`] and replays a whole, completed
//! utterance from memory the next time the same phrase is requested. It is applied by
//! [`crate::VoiceAgent`] only (see [`wrap_tts_for_agent`]), so shared speech pods that
//! call [`crate::VendorRegistry::create_tts`] directly never hold this cache.
//!
//! Rules:
//!
//! - Only complete syntheses are stored. A cancelled, cut-short or failed synthesis is
//!   never cached (see [`TtsProvider::synthesize_progressive_with_status`]).
//! - The cache is a byte-budget LRU, not an entry-count LRU: a long phrase costs what it
//!   weighs.
//! - Keys carry project, vendor, model, model path, voice and normalised text, so tenants
//!   and voices never share an entry.
//!
//! Environment (read once per process):
//!
//! | Variable | Default | Meaning |
//! | -------- | ------- | ------- |
//! | `VOICE_TTS_PHRASE_CACHE` | on | `0` / `false` / `no` / `off` disables |
//! | `VOICE_TTS_PHRASE_CACHE_MAX_BYTES` | 16 MiB | total PCM budget |
//! | `VOICE_TTS_PHRASE_CACHE_MAX_ENTRY_BYTES` | 2 MiB | larger phrases are not cached |
//! | `VOICE_TTS_PHRASE_CACHE_VENDORS` | `local-sherpa,cluster-sherpa` | comma list of TTS vendors |

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};

use crate::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use crate::error::SpeechResult;
use crate::otel::{self, SherpaTtsMetricAttrs};
use crate::pcm::{duration_ms_from_mono_s16le, WEBRTC_PCM_SAMPLE_RATE};
use crate::pipeline::{TtsAudioChunk, TtsProgressiveSink, TtsProvider, TtsSynthesis};

/// Upper bound for one progressive sink chunk: 1 s of stereo 48 kHz s16le
/// (a multiple of 20 ms of stereo audio). Keeps every gRPC message small.
pub const SINK_SLICE_MAX_BYTES: usize = 192_000;

/// Split `pcm` into zero-copy pieces of at most [`SINK_SLICE_MAX_BYTES`].
pub fn slice_for_sink(pcm: &Bytes) -> impl Iterator<Item = Bytes> + '_ {
    (0..pcm.len())
        .step_by(SINK_SLICE_MAX_BYTES)
        .map(move |start| pcm.slice(start..(start + SINK_SLICE_MAX_BYTES).min(pcm.len())))
}

/// Normalize phrase text for cache keys (trim + collapse whitespace).
pub fn normalize_phrase_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_VENDORS: &str = "local-sherpa,cluster-sherpa";

/// Cache key: everything that changes the audio for the same text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PhraseKey {
    project_id: String,
    vendor: &'static str,
    model: String,
    model_path: String,
    voice: String,
    text: String,
}

impl PhraseKey {
    pub fn new(cfg: &TtsConfig, project_id: &str, normalized: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            vendor: cfg.provider.as_str(),
            model: cfg.model.clone().unwrap_or_default(),
            model_path: cfg.model_path.clone().unwrap_or_default(),
            voice: cfg.voice.clone().unwrap_or_default(),
            text: normalized.to_string(),
        }
    }

    /// Same project, vendor, model, model path and voice (any text).
    fn same_group(&self, other: &Self) -> bool {
        self.project_id == other.project_id
            && self.vendor == other.vendor
            && self.model == other.model
            && self.model_path == other.model_path
            && self.voice == other.voice
    }
}

/// Phrase cache settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheSettings {
    pub enabled: bool,
    pub max_bytes: usize,
    pub max_entry_bytes: usize,
    pub vendors: Vec<String>,
}

impl CacheSettings {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Parse settings through `lookup` (no process-env access; used by tests).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let enabled = !matches!(
            lookup("VOICE_TTS_PHRASE_CACHE")
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("0") | Some("false") | Some("no") | Some("off")
        );
        let number = |key: &str, default: usize| {
            lookup(key)
                .and_then(|value| value.trim().parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        let parse_vendors = |raw: &str| -> Vec<String> {
            raw.split(',')
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect()
        };
        let mut vendors = lookup("VOICE_TTS_PHRASE_CACHE_VENDORS")
            .map(|raw| parse_vendors(&raw))
            .unwrap_or_default();
        if vendors.is_empty() {
            vendors = parse_vendors(DEFAULT_VENDORS);
        }
        Self {
            enabled,
            max_bytes: number("VOICE_TTS_PHRASE_CACHE_MAX_BYTES", DEFAULT_MAX_BYTES),
            max_entry_bytes: number(
                "VOICE_TTS_PHRASE_CACHE_MAX_ENTRY_BYTES",
                DEFAULT_MAX_ENTRY_BYTES,
            ),
            vendors,
        }
    }

    pub fn caches_vendor(&self, vendor: TtsVendor) -> bool {
        self.vendors.iter().any(|item| item == vendor.as_str())
    }
}

fn process_settings() -> &'static CacheSettings {
    static SETTINGS: OnceLock<CacheSettings> = OnceLock::new();
    SETTINGS.get_or_init(CacheSettings::from_env)
}

struct Entry {
    value: TtsAudioChunk,
    stamp: u64,
}

#[derive(Default)]
struct State {
    map: HashMap<PhraseKey, Entry>,
    /// Recency order: lowest stamp is the oldest entry.
    order: BTreeMap<u64, PhraseKey>,
    bytes: usize,
    next_stamp: u64,
}

impl State {
    fn bump(&mut self) -> u64 {
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        stamp
    }
}

/// Thread-safe LRU of completed phrases, bounded by total PCM bytes.
pub struct PhraseCache {
    state: Mutex<State>,
    max_bytes: usize,
    max_entry_bytes: usize,
}

impl PhraseCache {
    pub fn new(max_bytes: usize, max_entry_bytes: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            max_bytes,
            max_entry_bytes,
        }
    }

    /// Process-wide cache, sized from the environment on first use.
    pub fn global() -> Arc<PhraseCache> {
        static GLOBAL: OnceLock<Arc<PhraseCache>> = OnceLock::new();
        Arc::clone(GLOBAL.get_or_init(|| {
            let settings = process_settings();
            Arc::new(PhraseCache::new(
                settings.max_bytes,
                settings.max_entry_bytes,
            ))
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Look up `key` and mark it most recently used.
    pub fn get(&self, key: &PhraseKey) -> Option<TtsAudioChunk> {
        let mut state = self.lock();
        let stamp = state.bump();
        let entry = state.map.get_mut(key)?;
        let old = std::mem::replace(&mut entry.stamp, stamp);
        let value = entry.value.clone();
        state.order.remove(&old);
        state.order.insert(stamp, key.clone());
        Some(value)
    }

    /// Store `value`. Empty and over-limit phrases are skipped. Evicts the least recently
    /// used entries until the byte budget holds.
    pub fn insert(&self, key: PhraseKey, value: TtsAudioChunk) {
        let len = value.pcm.len();
        if len == 0 || len > self.max_entry_bytes {
            return;
        }
        let mut state = self.lock();
        if let Some(old) = state.map.remove(&key) {
            state.order.remove(&old.stamp);
            state.bytes -= old.value.pcm.len();
        }
        let stamp = state.bump();
        state.order.insert(stamp, key.clone());
        state.map.insert(key, Entry { value, stamp });
        state.bytes += len;
        while state.bytes > self.max_bytes {
            let Some((_, oldest)) = state.order.pop_first() else {
                break;
            };
            if let Some(evicted) = state.map.remove(&oldest) {
                state.bytes -= evicted.value.pcm.len();
            }
        }
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Entries and bytes held for the same project / vendor / model / voice as `key`.
    fn group_stats(&self, key: &PhraseKey) -> (usize, usize) {
        let state = self.lock();
        state
            .map
            .iter()
            .filter(|(candidate, _)| candidate.same_group(key))
            .fold((0, 0), |(count, bytes), (_, entry)| {
                (count + 1, bytes + entry.value.pcm.len())
            })
    }
}

/// Caches completed syntheses of the wrapped provider.
pub struct CachingTtsProvider {
    inner: Box<dyn TtsProvider>,
    config: TtsConfig,
    cache: Arc<PhraseCache>,
    project_id: Mutex<String>,
}

impl CachingTtsProvider {
    pub fn new(inner: Box<dyn TtsProvider>, config: TtsConfig, cache: Arc<PhraseCache>) -> Self {
        Self {
            inner,
            config,
            cache,
            project_id: Mutex::new(String::new()),
        }
    }

    fn current_project_id(&self) -> String {
        self.project_id
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl TtsProvider for CachingTtsProvider {
    fn vendor_name(&self) -> &'static str {
        self.inner.vendor_name()
    }

    fn bind_session_context(&self, ctx: &VoiceSessionContext) {
        let project_id = ctx
            .project_id
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if let Ok(mut guard) = self.project_id.lock() {
            *guard = project_id;
        }
        self.inner.bind_session_context(ctx);
    }

    async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
        self.synthesize_progressive(text, None).await
    }

    async fn synthesize_progressive(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<Vec<TtsAudioChunk>> {
        Ok(self
            .synthesize_progressive_with_status(text, sink)
            .await?
            .chunks)
    }

    async fn synthesize_progressive_with_status(
        &self,
        text: &str,
        sink: Option<TtsProgressiveSink>,
    ) -> SpeechResult<TtsSynthesis> {
        let normalized = normalize_phrase_text(text);
        if normalized.is_empty() {
            return Ok(TtsSynthesis {
                chunks: Vec::new(),
                complete: true,
            });
        }
        let project_id = self.current_project_id();
        let key = PhraseKey::new(&self.config, &project_id, &normalized);
        let attrs = SherpaTtsMetricAttrs::from_tts_config(&self.config, &project_id);

        if let Some(chunk) = self.cache.get(&key) {
            otel::record_sherpa_tts_phrase_cache_hit(&attrs);
            if let Some(sink) = sink {
                // The cached clip is the whole utterance; deliver it in bounded slices.
                for slice in slice_for_sink(&chunk.pcm) {
                    let duration_ms =
                        duration_ms_from_mono_s16le(slice.len() / 2, WEBRTC_PCM_SAMPLE_RATE).max(1);
                    if !sink.send(TtsAudioChunk {
                        pcm: slice,
                        duration_ms,
                    }) {
                        break;
                    }
                }
            }
            return Ok(TtsSynthesis {
                chunks: vec![chunk],
                complete: true,
            });
        }

        otel::record_sherpa_tts_phrase_cache_miss(&attrs);
        let out = self
            .inner
            .synthesize_progressive_with_status(text, sink)
            .await?;
        if out.complete {
            if let Some(chunk) = concat_chunks(&out.chunks) {
                self.cache.insert(key.clone(), chunk);
                let (entries, bytes) = self.cache.group_stats(&key);
                otel::set_sherpa_tts_phrase_cache_entries(entries as i64, &attrs);
                otel::set_sherpa_tts_phrase_cache_bytes(bytes as i64, &attrs);
            }
        }
        Ok(out)
    }
}

/// One chunk holding all of `chunks`; `None` when there is no audio.
fn concat_chunks(chunks: &[TtsAudioChunk]) -> Option<TtsAudioChunk> {
    let total: usize = chunks.iter().map(|chunk| chunk.pcm.len()).sum();
    if total == 0 {
        return None;
    }
    let duration_ms: u32 = chunks.iter().map(|chunk| chunk.duration_ms).sum();
    let nonempty = chunks.iter().filter(|chunk| !chunk.pcm.is_empty());
    let mut only = None;
    let mut count = 0;
    for chunk in nonempty {
        count += 1;
        only = Some(chunk);
    }
    let pcm = if count == 1 {
        // Single piece: share it, no copy.
        only.map(|chunk| chunk.pcm.clone()).unwrap_or_default()
    } else {
        let mut buf = BytesMut::with_capacity(total);
        for chunk in chunks {
            buf.extend_from_slice(&chunk.pcm);
        }
        buf.freeze()
    };
    Some(TtsAudioChunk { pcm, duration_ms })
}

/// Wrap `inner` with the phrase cache when `settings` enable it for `config.provider`.
pub fn wrap_for(
    settings: &CacheSettings,
    cache: &Arc<PhraseCache>,
    inner: Box<dyn TtsProvider>,
    config: &TtsConfig,
) -> Box<dyn TtsProvider> {
    if settings.enabled && settings.caches_vendor(config.provider) {
        Box::new(CachingTtsProvider::new(
            inner,
            config.clone(),
            Arc::clone(cache),
        ))
    } else {
        inner
    }
}

/// Wrap a TTS provider for use by [`crate::VoiceAgent`], using process environment settings.
pub fn wrap_tts_for_agent(inner: Box<dyn TtsProvider>, config: &TtsConfig) -> Box<dyn TtsProvider> {
    let settings = process_settings();
    if !(settings.enabled && settings.caches_vendor(config.provider)) {
        return inner;
    }
    wrap_for(settings, &PhraseCache::global(), inner, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::mpsc;

    fn cfg(provider: TtsVendor, voice: &str) -> TtsConfig {
        TtsConfig {
            provider,
            model: Some("m".into()),
            model_path: Some("/models/tts".into()),
            voice: Some(voice.into()),
            api_key: None,
            endpoint: None,
        }
    }

    fn pcm_of(len: usize) -> Bytes {
        Bytes::from((0..len).map(|i| (i % 251) as u8).collect::<Vec<u8>>())
    }

    #[derive(Default)]
    struct FakeState {
        calls: AtomicUsize,
        incomplete: AtomicBool,
        fail: AtomicBool,
    }

    /// Reports completeness itself (like Sherpa / cluster do).
    struct Fake {
        state: Arc<FakeState>,
        pcm_len: usize,
    }

    impl Fake {
        fn boxed(pcm_len: usize) -> (Box<dyn TtsProvider>, Arc<FakeState>) {
            let state = Arc::new(FakeState::default());
            (
                Box::new(Fake {
                    state: Arc::clone(&state),
                    pcm_len,
                }),
                state,
            )
        }

        fn chunks(&self) -> Vec<TtsAudioChunk> {
            let half = self.pcm_len / 2;
            vec![
                TtsAudioChunk {
                    pcm: pcm_of(half),
                    duration_ms: 10,
                },
                TtsAudioChunk {
                    pcm: pcm_of(self.pcm_len - half),
                    duration_ms: 10,
                },
            ]
        }
    }

    #[async_trait]
    impl TtsProvider for Fake {
        fn vendor_name(&self) -> &'static str {
            "fake"
        }

        async fn synthesize(&self, text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            self.synthesize_progressive(text, None).await
        }

        async fn synthesize_progressive(
            &self,
            text: &str,
            sink: Option<TtsProgressiveSink>,
        ) -> SpeechResult<Vec<TtsAudioChunk>> {
            Ok(self
                .synthesize_progressive_with_status(text, sink)
                .await?
                .chunks)
        }

        async fn synthesize_progressive_with_status(
            &self,
            _text: &str,
            sink: Option<TtsProgressiveSink>,
        ) -> SpeechResult<TtsSynthesis> {
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            if self.state.fail.load(Ordering::SeqCst) {
                return Err(crate::error::SpeechError::Vendor {
                    vendor: "fake".into(),
                    message: "boom".into(),
                });
            }
            let chunks = self.chunks();
            if let Some(sink) = sink {
                for chunk in &chunks {
                    sink.send(chunk.clone());
                }
            }
            Ok(TtsSynthesis {
                chunks,
                complete: !self.state.incomplete.load(Ordering::SeqCst),
            })
        }
    }

    /// Does not override `_with_status`: relies on the trait default, which reads the
    /// sink's cancel flag. Cancels the sink during synthesis.
    struct FakeDefault {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TtsProvider for FakeDefault {
        fn vendor_name(&self) -> &'static str {
            "fake-default"
        }

        async fn synthesize(&self, _text: &str) -> SpeechResult<Vec<TtsAudioChunk>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![TtsAudioChunk {
                pcm: pcm_of(1_000),
                duration_ms: 10,
            }])
        }

        async fn synthesize_progressive(
            &self,
            text: &str,
            sink: Option<TtsProgressiveSink>,
        ) -> SpeechResult<Vec<TtsAudioChunk>> {
            if let Some(sink) = sink.as_ref() {
                sink.cancel.store(true, Ordering::SeqCst);
            }
            self.synthesize(text).await
        }
    }

    fn sink() -> (
        TtsProgressiveSink,
        mpsc::UnboundedReceiver<TtsAudioChunk>,
        Arc<AtomicBool>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = Arc::new(AtomicBool::new(false));
        (
            TtsProgressiveSink {
                tx,
                cancel: Arc::clone(&cancel),
            },
            rx,
            cancel,
        )
    }

    fn total(chunks: &[TtsAudioChunk]) -> Vec<u8> {
        chunks.iter().flat_map(|c| c.pcm.iter().copied()).collect()
    }

    fn wrapper(
        inner: Box<dyn TtsProvider>,
        config: &TtsConfig,
        cache: &Arc<PhraseCache>,
    ) -> CachingTtsProvider {
        CachingTtsProvider::new(inner, config.clone(), Arc::clone(cache))
    }

    fn big_cache() -> Arc<PhraseCache> {
        Arc::new(PhraseCache::new(16 * 1024 * 1024, 2 * 1024 * 1024))
    }

    fn bind(provider: &CachingTtsProvider, project: &str) {
        provider.bind_session_context(&VoiceSessionContext {
            project_id: Some(project.into()),
            ..Default::default()
        });
    }

    #[tokio::test]
    async fn hit_skips_inner_and_returns_same_pcm() {
        let (inner, state) = Fake::boxed(4_000);
        let cache = big_cache();
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        let first = tts.synthesize("hello").await.unwrap();
        let second = tts.synthesize("hello").await.unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
        assert_eq!(total(&first), total(&second));
        assert_eq!(second.len(), 1);
    }

    #[tokio::test]
    async fn whitespace_variants_share_entry() {
        let (inner, state) = Fake::boxed(4_000);
        let cache = big_cache();
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        tts.synthesize("Hello  world ").await.unwrap();
        tts.synthesize("Hello world").await.unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn incomplete_synthesis_not_cached() {
        let (inner, state) = Fake::boxed(4_000);
        state.incomplete.store(true, Ordering::SeqCst);
        let cache = big_cache();
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        let first = tts
            .synthesize_progressive_with_status("hello", None)
            .await
            .unwrap();
        assert!(!first.complete);
        tts.synthesize("hello").await.unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 2);
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn error_not_cached() {
        let (inner, state) = Fake::boxed(4_000);
        state.fail.store(true, Ordering::SeqCst);
        let cache = big_cache();
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        assert!(tts.synthesize("hello").await.is_err());
        state.fail.store(false, Ordering::SeqCst);
        tts.synthesize("hello").await.unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn cancelled_sink_not_cached_with_default_status() {
        let calls = Arc::new(AtomicUsize::new(0));
        let inner: Box<dyn TtsProvider> = Box::new(FakeDefault {
            calls: Arc::clone(&calls),
        });
        let cache = big_cache();
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        let (sink_a, _rx_a, _) = sink();
        let first = tts
            .synthesize_progressive_with_status("hello", Some(sink_a))
            .await
            .unwrap();
        assert!(!first.complete, "default status sees the cancelled sink");
        let (sink_b, _rx_b, _) = sink();
        tts.synthesize_progressive("hello", Some(sink_b))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(cache.is_empty());
    }

    fn key(text: &str) -> PhraseKey {
        PhraseKey::new(&cfg(TtsVendor::LocalSherpa, "a"), "p", text)
    }

    fn chunk(len: usize) -> TtsAudioChunk {
        TtsAudioChunk {
            pcm: pcm_of(len),
            duration_ms: 1,
        }
    }

    #[tokio::test]
    async fn byte_budget_evicts_oldest() {
        let (inner, state) = Fake::boxed(4_000);
        let cache = Arc::new(PhraseCache::new(10_000, 10_000));
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        for text in ["A", "B", "C"] {
            tts.synthesize(text).await.unwrap();
        }
        assert_eq!(state.calls.load(Ordering::SeqCst), 3);
        assert!(cache.bytes() <= 10_000);
        assert!(cache
            .get(&PhraseKey::new(&cfg(TtsVendor::LocalSherpa, "a"), "", "A"))
            .is_none());
        assert!(cache
            .get(&PhraseKey::new(&cfg(TtsVendor::LocalSherpa, "a"), "", "B"))
            .is_some());
        assert!(cache
            .get(&PhraseKey::new(&cfg(TtsVendor::LocalSherpa, "a"), "", "C"))
            .is_some());
        tts.synthesize("B").await.unwrap();
        tts.synthesize("C").await.unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 3, "B and C hit");
    }

    #[test]
    fn get_refreshes_recency() {
        let cache = PhraseCache::new(10_000, 10_000);
        cache.insert(key("A"), chunk(4_000));
        cache.insert(key("B"), chunk(4_000));
        assert!(cache.get(&key("A")).is_some());
        cache.insert(key("C"), chunk(4_000));
        assert!(cache.get(&key("B")).is_none(), "B was least recently used");
        assert!(cache.get(&key("A")).is_some());
        assert!(cache.get(&key("C")).is_some());
        assert!(cache.bytes() <= 10_000);
    }

    #[tokio::test]
    async fn entry_over_limit_not_cached() {
        let (inner, state) = Fake::boxed(4_000);
        let cache = Arc::new(PhraseCache::new(1_000_000, 1_000));
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        tts.synthesize("hello").await.unwrap();
        tts.synthesize("hello").await.unwrap();
        assert_eq!(state.calls.load(Ordering::SeqCst), 2);
        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn hit_delivered_in_bounded_slices() {
        let (inner, _state) = Fake::boxed(500_000);
        let cache = big_cache();
        let tts = wrapper(inner, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        let original = tts.synthesize("hello").await.unwrap();
        let (sink, mut rx, _) = sink();
        let out = tts
            .synthesize_progressive("hello", Some(sink))
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        let mut sizes = Vec::new();
        let mut joined = Vec::new();
        while let Ok(slice) = rx.try_recv() {
            sizes.push(slice.pcm.len());
            joined.extend_from_slice(&slice.pcm);
        }
        assert_eq!(sizes, vec![192_000, 192_000, 116_000]);
        assert_eq!(joined, total(&original));
    }

    #[tokio::test]
    async fn different_project_ids_do_not_share_cached_phrase() {
        let cache = big_cache();
        let config = cfg(TtsVendor::LocalSherpa, "a");
        let (inner_a, state_a) = Fake::boxed(4_000);
        let tts_a = wrapper(inner_a, &config, &cache);
        bind(&tts_a, "project-a");
        tts_a.synthesize("hello").await.unwrap();
        let (inner_b, state_b) = Fake::boxed(4_000);
        let tts_b = wrapper(inner_b, &config, &cache);
        bind(&tts_b, "project-b");
        tts_b.synthesize("hello").await.unwrap();
        assert_eq!(state_a.calls.load(Ordering::SeqCst), 1);
        assert_eq!(state_b.calls.load(Ordering::SeqCst), 1);
        // Same project does share.
        let (inner_c, state_c) = Fake::boxed(4_000);
        let tts_c = wrapper(inner_c, &config, &cache);
        bind(&tts_c, "project-a");
        tts_c.synthesize("hello").await.unwrap();
        assert_eq!(state_c.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn different_voice_or_model_do_not_share() {
        let cache = big_cache();
        let (inner_a, state_a) = Fake::boxed(4_000);
        let tts_a = wrapper(inner_a, &cfg(TtsVendor::LocalSherpa, "a"), &cache);
        tts_a.synthesize("hello").await.unwrap();
        let (inner_b, state_b) = Fake::boxed(4_000);
        let tts_b = wrapper(inner_b, &cfg(TtsVendor::LocalSherpa, "b"), &cache);
        tts_b.synthesize("hello").await.unwrap();
        let mut other_model = cfg(TtsVendor::LocalSherpa, "a");
        other_model.model = Some("other".into());
        let (inner_c, state_c) = Fake::boxed(4_000);
        let tts_c = wrapper(inner_c, &other_model, &cache);
        tts_c.synthesize("hello").await.unwrap();
        assert_eq!(state_a.calls.load(Ordering::SeqCst), 1);
        assert_eq!(state_b.calls.load(Ordering::SeqCst), 1);
        assert_eq!(state_c.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn settings_from_env_defaults_and_overrides() {
        let none = |_: &str| None;
        let defaults = CacheSettings::from_lookup(none);
        assert!(defaults.enabled);
        assert_eq!(defaults.max_bytes, 16 * 1024 * 1024);
        assert_eq!(defaults.max_entry_bytes, 2 * 1024 * 1024);
        assert_eq!(defaults.vendors, vec!["local-sherpa", "cluster-sherpa"]);
        assert!(defaults.caches_vendor(TtsVendor::LocalSherpa));
        assert!(defaults.caches_vendor(TtsVendor::ClusterSherpa));
        assert!(!defaults.caches_vendor(TtsVendor::Openai));

        for off in ["0", "false", "no", "off", " OFF "] {
            let settings = CacheSettings::from_lookup(|key| {
                (key == "VOICE_TTS_PHRASE_CACHE").then(|| off.to_string())
            });
            assert!(!settings.enabled, "{off:?} disables");
        }

        let custom = CacheSettings::from_lookup(|key| match key {
            "VOICE_TTS_PHRASE_CACHE_MAX_BYTES" => Some(" 1000 ".into()),
            "VOICE_TTS_PHRASE_CACHE_MAX_ENTRY_BYTES" => Some("500".into()),
            "VOICE_TTS_PHRASE_CACHE_VENDORS" => Some(" OpenAI , local-sherpa ,, ".into()),
            _ => None,
        });
        assert_eq!(custom.max_bytes, 1000);
        assert_eq!(custom.max_entry_bytes, 500);
        assert_eq!(custom.vendors, vec!["openai", "local-sherpa"]);
        assert!(custom.caches_vendor(TtsVendor::Openai));
        assert!(!custom.caches_vendor(TtsVendor::ClusterSherpa));

        for bad in ["0", "-5", "abc", ""] {
            let settings = CacheSettings::from_lookup(|key| match key {
                "VOICE_TTS_PHRASE_CACHE_MAX_BYTES" | "VOICE_TTS_PHRASE_CACHE_MAX_ENTRY_BYTES" => {
                    Some(bad.to_string())
                }
                _ => None,
            });
            assert_eq!(settings.max_bytes, 16 * 1024 * 1024, "{bad:?}");
            assert_eq!(settings.max_entry_bytes, 2 * 1024 * 1024, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn wrap_only_for_listed_vendors() {
        let settings = CacheSettings::from_lookup(|_| None);
        let cache = big_cache();

        let (inner, state) = Fake::boxed(4_000);
        let wrapped = wrap_for(&settings, &cache, inner, &cfg(TtsVendor::Openai, "a"));
        assert_eq!(wrapped.vendor_name(), "fake");
        wrapped.synthesize("hello").await.unwrap();
        wrapped.synthesize("hello").await.unwrap();
        assert_eq!(
            state.calls.load(Ordering::SeqCst),
            2,
            "openai is not cached"
        );
        assert!(cache.is_empty());

        let (inner, state) = Fake::boxed(4_000);
        let wrapped = wrap_for(&settings, &cache, inner, &cfg(TtsVendor::LocalSherpa, "a"));
        wrapped.synthesize("hello").await.unwrap();
        wrapped.synthesize("hello").await.unwrap();
        assert_eq!(
            state.calls.load(Ordering::SeqCst),
            1,
            "local-sherpa is cached"
        );

        let disabled = CacheSettings::from_lookup(|key| {
            (key == "VOICE_TTS_PHRASE_CACHE").then(|| "off".to_string())
        });
        let (inner, state) = Fake::boxed(4_000);
        let wrapped = wrap_for(
            &disabled,
            &cache,
            inner,
            &cfg(TtsVendor::ClusterSherpa, "a"),
        );
        wrapped.synthesize("again").await.unwrap();
        wrapped.synthesize("again").await.unwrap();
        assert_eq!(
            state.calls.load(Ordering::SeqCst),
            2,
            "disabled caches nothing"
        );
    }

    #[test]
    fn normalize_phrase_text_trims_and_collapses_whitespace() {
        assert_eq!(normalize_phrase_text("  hello   world  "), "hello world");
    }

    #[test]
    fn slice_for_sink_splits_at_one_second() {
        let pcm = pcm_of(SINK_SLICE_MAX_BYTES * 2 + 10);
        let sizes: Vec<usize> = slice_for_sink(&pcm).map(|s| s.len()).collect();
        assert_eq!(sizes, vec![SINK_SLICE_MAX_BYTES, SINK_SLICE_MAX_BYTES, 10]);
    }
}
