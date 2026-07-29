//! In-memory LRU cache for Sherpa offline TTS phrase PCM.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use node_webrtc_rust_speech::otel::{self, SherpaTtsMetricAttrs};
use node_webrtc_rust_speech::pipeline::TtsAudioChunk;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PhraseCacheKey {
    pub project_id: String,
    pub model_dir: String,
    pub language: String,
    pub voice: String,
    pub normalized_text: String,
}

struct LruEntry {
    value: TtsAudioChunk,
}

struct PhraseCacheState {
    map: HashMap<PhraseCacheKey, LruEntry>,
    order: Vec<PhraseCacheKey>,
    max_entries: usize,
}

impl PhraseCacheState {
    fn new(max_entries: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
            max_entries: max_entries.max(1),
        }
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn count_for_attrs(
        &self,
        project_id: &str,
        model_dir: &str,
        language: &str,
        voice: &str,
    ) -> usize {
        self.map
            .keys()
            .filter(|key| {
                key.project_id == project_id
                    && key.model_dir == model_dir
                    && key.language == language
                    && key.voice == voice
            })
            .count()
    }

    fn touch(&mut self, key: &PhraseCacheKey) {
        if let Some(index) = self.order.iter().position(|existing| existing == key) {
            self.order.remove(index);
        }
        self.order.push(key.clone());
    }

    fn evict_if_needed(&mut self) {
        while self.map.len() > self.max_entries {
            let Some(oldest) = self.order.first().cloned() else {
                break;
            };
            self.order.remove(0);
            self.map.remove(&oldest);
        }
    }

    fn get(&mut self, key: &PhraseCacheKey) -> Option<TtsAudioChunk> {
        if !self.map.contains_key(key) {
            return None;
        }
        let entry = self.map.get(key)?.value.clone();
        self.touch(key);
        Some(entry)
    }

    fn insert(&mut self, key: PhraseCacheKey, value: TtsAudioChunk) {
        if self.map.contains_key(&key) {
            self.map.insert(key.clone(), LruEntry { value });
            self.touch(&key);
            return;
        }
        self.map.insert(key.clone(), LruEntry { value });
        self.touch(&key);
        self.evict_if_needed();
    }
}

static PHRASE_CACHE: OnceLock<Mutex<PhraseCacheState>> = OnceLock::new();

fn global_cache() -> &'static Mutex<PhraseCacheState> {
    PHRASE_CACHE.get_or_init(|| Mutex::new(PhraseCacheState::new(phrase_cache_max_entries())))
}

pub fn phrase_cache_enabled() -> bool {
    match std::env::var("SHERPA_TTS_PHRASE_CACHE")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        _ => true,
    }
}

pub fn phrase_cache_max_entries() -> usize {
    std::env::var("SHERPA_TTS_PHRASE_CACHE_MAX_ENTRIES")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(128)
}

/// Normalize phrase text for cache keys (trim + collapse whitespace).
pub fn normalize_phrase_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn lookup(key: &PhraseCacheKey, attrs: &SherpaTtsMetricAttrs) -> Option<TtsAudioChunk> {
    if !phrase_cache_enabled() {
        return None;
    }
    let mut cache = global_cache().lock().expect("phrase cache lock poisoned");
    let hit = cache.get(key);
    if hit.is_some() {
        otel::record_sherpa_tts_phrase_cache_hit(attrs);
    }
    hit
}

pub fn store(key: PhraseCacheKey, chunk: TtsAudioChunk, attrs: &SherpaTtsMetricAttrs) {
    if !phrase_cache_enabled() {
        return;
    }
    let mut cache = global_cache().lock().expect("phrase cache lock poisoned");
    cache.insert(key.clone(), chunk);
    let count = cache.count_for_attrs(
        &key.project_id,
        &key.model_dir,
        &key.language,
        &key.voice,
    );
    otel::set_sherpa_tts_phrase_cache_entries(count as i64, attrs);
}

pub fn build_cache_key(
    project_id: &str,
    model_dir: &str,
    language: &str,
    voice: &str,
    text: &str,
) -> PhraseCacheKey {
    PhraseCacheKey {
        project_id: project_id.to_string(),
        model_dir: model_dir.to_string(),
        language: language.to_string(),
        voice: voice.to_string(),
        normalized_text: normalize_phrase_text(text),
    }
}

pub fn build_metric_attrs(
    project_id: &str,
    model_id: &str,
    model_dir: &str,
    language: &str,
    voice: &str,
) -> SherpaTtsMetricAttrs {
    let model_dir_basename = otel::path_basename(model_dir);
    let catalog_id = model_id.trim();
    SherpaTtsMetricAttrs {
        tts_vendor: "local-sherpa".to_string(),
        tts_model: if catalog_id.is_empty() {
            model_dir_basename.clone()
        } else {
            catalog_id.to_string()
        },
        tts_model_dir: model_dir_basename,
        tts_language: language.to_string(),
        tts_voice: voice.to_string(),
        project_id: project_id.to_string(),
    }
}

#[cfg(test)]
pub fn reset_for_test(max_entries: usize) {
    let mut cache = global_cache().lock().expect("phrase cache lock poisoned");
    cache.map.clear();
    cache.order.clear();
    cache.max_entries = max_entries.max(1);
    otel::set_sherpa_tts_phrase_cache_entries(
        0,
        &SherpaTtsMetricAttrs {
            tts_vendor: String::new(),
            tts_model: String::new(),
            tts_model_dir: String::new(),
            tts_language: String::new(),
            tts_voice: String::new(),
            project_id: String::new(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::sync::{Mutex, OnceLock};

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("phrase cache test lock")
    }

    fn sample_chunk(label: &str) -> TtsAudioChunk {
        TtsAudioChunk {
            pcm: Bytes::from(label.as_bytes().to_vec()),
            duration_ms: 100,
        }
    }

    fn attrs() -> SherpaTtsMetricAttrs {
        SherpaTtsMetricAttrs {
            tts_vendor: "local-sherpa".into(),
            tts_model: "en-amy-medium".into(),
            tts_model_dir: "piper".into(),
            tts_language: String::new(),
            tts_voice: "0".into(),
            project_id: "proj-a".into(),
        }
    }

    #[test]
    fn build_metric_attrs_prefers_catalog_model_id() {
        let attrs = build_metric_attrs(
            "proj-a",
            "en-amy-medium",
            "/models/vits-piper-en_US-amy-medium",
            "",
            "0",
        );
        assert_eq!(attrs.tts_model, "en-amy-medium");
        assert_eq!(attrs.tts_model_dir, "vits-piper-en_US-amy-medium");
        assert_eq!(attrs.tts_vendor, "local-sherpa");
    }

    #[test]
    fn build_metric_attrs_falls_back_to_dir_basename() {
        let attrs = build_metric_attrs("proj-a", "", "/models/vits-piper-en_US-amy-medium", "", "0");
        assert_eq!(attrs.tts_model, "vits-piper-en_US-amy-medium");
        assert_eq!(attrs.tts_model_dir, "vits-piper-en_US-amy-medium");
    }

    #[test]
    fn normalize_phrase_text_trims_and_collapses_whitespace() {
        let _lock = test_lock();
        assert_eq!(normalize_phrase_text("  hello   world  "), "hello world");
    }

    #[test]
    fn cache_hit_returns_same_pcm_without_second_insert() {
        let _lock = test_lock();
        reset_for_test(128);
        let key = build_cache_key("proj-a", "/models/piper", "", "0", "hello");
        let attrs = attrs();
        store(key.clone(), sample_chunk("first"), &attrs);
        let hit = lookup(&key, &attrs).expect("cache hit");
        assert_eq!(hit.pcm.as_ref(), b"first");
    }

    #[test]
    fn different_texts_are_distinct_entries() {
        let _lock = test_lock();
        reset_for_test(128);
        let attrs = attrs();
        let key_a = build_cache_key("proj-a", "/models/piper", "", "0", "hello");
        let key_b = build_cache_key("proj-a", "/models/piper", "", "0", "goodbye");
        store(key_a.clone(), sample_chunk("a"), &attrs);
        store(key_b.clone(), sample_chunk("b"), &attrs);
        assert_eq!(lookup(&key_a, &attrs).expect("a").pcm.as_ref(), b"a");
        assert_eq!(lookup(&key_b, &attrs).expect("b").pcm.as_ref(), b"b");
    }

    #[test]
    fn different_project_ids_do_not_share_entries() {
        let _lock = test_lock();
        reset_for_test(128);
        let attrs = attrs();
        let key_a = build_cache_key("proj-a", "/models/piper", "", "0", "hello");
        let key_b = build_cache_key("proj-b", "/models/piper", "", "0", "hello");
        store(key_a.clone(), sample_chunk("a"), &attrs);
        store(key_b.clone(), sample_chunk("b"), &attrs);
        assert_eq!(lookup(&key_a, &attrs).expect("a").pcm.as_ref(), b"a");
        assert_eq!(lookup(&key_b, &attrs).expect("b").pcm.as_ref(), b"b");
    }

    #[test]
    fn lru_evicts_oldest_entry() {
        let _lock = test_lock();
        reset_for_test(2);
        let attrs = attrs();
        let key_a = build_cache_key("proj-a", "/models/piper", "", "0", "one");
        let key_b = build_cache_key("proj-a", "/models/piper", "", "0", "two");
        let key_c = build_cache_key("proj-a", "/models/piper", "", "0", "three");
        store(key_a.clone(), sample_chunk("a"), &attrs);
        store(key_b.clone(), sample_chunk("b"), &attrs);
        store(key_c.clone(), sample_chunk("c"), &attrs);
        assert!(lookup(&key_a, &attrs).is_none(), "oldest entry evicted");
        assert!(lookup(&key_b, &attrs).is_some());
        assert!(lookup(&key_c, &attrs).is_some());
    }

    #[test]
    fn phrase_cache_disabled_skips_lookup_and_store() {
        let _lock = test_lock();
        reset_for_test(128);
        unsafe { std::env::set_var("SHERPA_TTS_PHRASE_CACHE", "0") };
        assert!(!phrase_cache_enabled());
        let cache_key = build_cache_key("proj-a", "/models/piper", "", "0", "hello");
        let attrs = attrs();
        store(cache_key.clone(), sample_chunk("first"), &attrs);
        assert!(lookup(&cache_key, &attrs).is_none());
        unsafe { std::env::remove_var("SHERPA_TTS_PHRASE_CACHE") };
    }
}
