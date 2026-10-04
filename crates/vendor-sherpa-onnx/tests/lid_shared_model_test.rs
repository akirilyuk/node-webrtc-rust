//! Shared-LID-model integration tests (need Whisper tiny weights).
//!
//! `#[ignore]` for default `cargo test`; CI runs them via `scripts/ci/run-sherpa-example-ci.sh`
//! with `SHERPA_LID_MODEL_PATH` set. Run with `--test-threads=1`: the pool and the create
//! counter are process-global, so each test uses its own symlinked model directory (distinct
//! pool key) and compares counter deltas.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use node_webrtc_rust_speech::config::LanguageIdConfig;
use node_webrtc_rust_speech::pcm::i16_samples_to_bytes;
use node_webrtc_rust_speech::pipeline::{LanguageIdProvider, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{
    lid_model_create_count, preload_language_id, SherpaFactory, SherpaModelPool,
};

fn model_dir() -> PathBuf {
    PathBuf::from(std::env::var("SHERPA_LID_MODEL_PATH").expect("set SHERPA_LID_MODEL_PATH"))
}

/// A fresh directory of symlinks to the real model files: distinct pool key, same weights.
fn private_model_dir(label: &str) -> PathBuf {
    private_dir_from(&model_dir(), label)
}

fn private_dir_from(src: &Path, label: &str) -> PathBuf {
    let dst = std::env::temp_dir().join(format!("nwr-lid-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::create_dir_all(&dst).expect("mkdir");
    for entry in std::fs::read_dir(src).expect("read model dir") {
        let entry = entry.expect("entry");
        if entry.path().is_file() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(entry.path(), dst.join(entry.file_name())).expect("link");
            #[cfg(not(unix))]
            std::fs::copy(entry.path(), dst.join(entry.file_name())).expect("copy");
        }
    }
    dst
}

fn lid_config(dir: &Path) -> LanguageIdConfig {
    LanguageIdConfig {
        enabled: Some(true),
        model_path: Some(dir.to_str().expect("utf8").into()),
        allowlist: None,
        min_speech_ms: None,
        continuous: None,
        lid_max_clip_ms: None,
        lid_gate_max_wait_ms: None,
        timing: None,
        tts_exclusion: None,
    }
}

/// 16 kHz mono s16le speech from the Whisper bundle's own `test_wavs/0.wav` when present,
/// otherwise 3 s of a modulated tone (timing is what these tests measure).
fn clip_16k() -> Bytes {
    let wav = model_dir().join("test_wavs/0.wav");
    if let Ok(bytes) = std::fs::read(&wav) {
        if bytes.len() > 44 {
            return Bytes::copy_from_slice(&bytes[44..]);
        }
    }
    let samples: Vec<i16> = (0..48_000)
        .map(|i| {
            let t = i as f32 / 16_000.0;
            ((t * 220.0 * std::f32::consts::TAU).sin() * 6000.0) as i16
        })
        .collect();
    i16_samples_to_bytes(&samples)
}

#[test]
#[ignore = "requires SHERPA_LID_MODEL_PATH with Whisper tiny bundle"]
fn two_agents_same_lid_path_share_one_loaded_model() {
    let dir = private_model_dir("share");
    let config = lid_config(&dir);
    // Same directory spelled differently must still hit the same pool entry.
    let dotted = lid_config(&dir.join("."));
    let pool = SherpaModelPool::global();
    let created_before = lid_model_create_count();
    let entries_before = pool.lid_entry_count();

    // What VoiceAgent::new does for each agent: ask the factory for a LID provider.
    let factory = SherpaFactory;
    let _agent_a = factory
        .create_language_id(&config)
        .expect("a")
        .expect("some");
    let _agent_b = factory
        .create_language_id(&dotted)
        .expect("b")
        .expect("some");
    preload_language_id(&config).expect("preload");
    pool.join_lid_preloads();

    assert_eq!(
        lid_model_create_count() - created_before,
        1,
        "two agents with the same LID path must load Whisper once"
    );
    assert_eq!(pool.lid_entry_count(), entries_before + 1);
    let ptr = pool.shared_lid_ptr(&config).expect("loaded");
    assert_eq!(Some(ptr), pool.shared_lid_ptr(&dotted));

    // A different model directory gets its own (bounded: one per distinct path) instance.
    let other = private_model_dir("share-other");
    preload_language_id(&lid_config(&other)).expect("other");
    assert_eq!(pool.lid_entry_count(), entries_before + 2);
    assert_eq!(lid_model_create_count() - created_before, 2);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SHERPA_LID_MODEL_PATH with Whisper tiny bundle"]
async fn concurrent_sessions_identify_safely_and_load_once() {
    let dir = private_model_dir("concurrent");
    let config = lid_config(&dir);
    let created_before = lid_model_create_count();
    let clip = clip_16k();

    // Cold: no preload awaited. Eight sessions race the first identify.
    let providers: Vec<Arc<dyn LanguageIdProvider>> = (0..8)
        .map(|_| {
            let boxed = SherpaFactory
                .create_language_id(&config)
                .expect("create")
                .expect("some");
            Arc::from(boxed)
        })
        .collect();
    let mut tasks = Vec::new();
    for provider in providers {
        let pcm = clip.clone();
        tasks.push(tokio::spawn(async move {
            provider.identify(pcm, 16_000).await.expect("identify")
        }));
    }
    let mut languages = Vec::new();
    for task in tasks {
        languages.push(task.await.expect("join").expect("language").language);
    }

    assert_eq!(
        lid_model_create_count() - created_before,
        1,
        "concurrent first callers must single-flight the load"
    );
    assert!(
        languages.windows(2).all(|pair| pair[0] == pair[1]),
        "same clip must give the same language on every session: {languages:?}"
    );
    SherpaModelPool::global().join_lid_preloads();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "requires SHERPA_LID_MODEL_PATH with Whisper tiny bundle"]
async fn first_lid_decision_does_not_include_model_load() {
    let dir = private_model_dir("first-decision");
    let config = lid_config(&dir);
    let clip = clip_16k();

    // Preload (what the runner calls at boot / what VoiceAgent::new starts in the background).
    let load_started = Instant::now();
    preload_language_id(&config).expect("preload");
    let load_ms = load_started.elapsed().as_millis();
    let created_after_preload = lid_model_create_count();

    let provider = SherpaFactory
        .create_language_id(&config)
        .expect("create")
        .expect("some");
    let first_started = Instant::now();
    let first = provider
        .identify(clip.clone(), 16_000)
        .await
        .expect("first");
    let first_ms = first_started.elapsed().as_millis();
    let second_started = Instant::now();
    let _ = provider.identify(clip, 16_000).await.expect("second");
    let second_ms = second_started.elapsed().as_millis();

    eprintln!(
        "LID timing: model_load_ms={load_ms} first_identify_ms={first_ms} \
         second_identify_ms={second_ms} language={:?}",
        first.as_ref().map(|r| r.language.as_str())
    );
    assert_eq!(
        lid_model_create_count(),
        created_after_preload,
        "identify after preload must not load the model again"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Lock-order regression: the pool used to lock `stt` then `lid` (and `lid` then `stt`) while
/// updating the entry gauge, so an STT recognizer load overlapping a LID load (LID now loads
/// in the background at agent construction) deadlocked the session before its first event.
#[test]
#[ignore = "requires SHERPA_LID_MODEL_PATH and SHERPA_STT_MODEL_PATH"]
fn stt_and_lid_loads_overlap_without_deadlock() {
    use node_webrtc_rust_speech::config::{SttConfig, SttVendor};

    let stt_src =
        PathBuf::from(std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH"));
    for round in 0..4 {
        let stt_dir = private_dir_from(&stt_src, &format!("deadlock-stt-{round}"));
        let lid_dir = private_model_dir(&format!("deadlock-lid-{round}"));
        let stt_config = SttConfig {
            provider: SttVendor::LocalSherpa,
            model: None,
            model_path: Some(stt_dir.to_str().expect("utf8").into()),
            language: Some("en".into()),
            api_key: None,
            endpoint: None,
        };
        let lid_config = lid_config(&lid_dir);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let pool = SherpaModelPool::global();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        {
            let (pool, barrier, tx) = (Arc::clone(&pool), Arc::clone(&barrier), done_tx.clone());
            std::thread::spawn(move || {
                barrier.wait();
                pool.get_or_create_stt(&stt_config).expect("stt");
                let _ = tx.send("stt");
            });
        }
        {
            let (pool, barrier, tx) = (Arc::clone(&pool), Arc::clone(&barrier), done_tx);
            std::thread::spawn(move || {
                barrier.wait();
                pool.preload_lid(&lid_config).expect("lid");
                let _ = tx.send("lid");
            });
        }
        for _ in 0..2 {
            done_rx
                .recv_timeout(std::time::Duration::from_secs(60))
                .expect("STT and LID loads must both finish (pool lock-order deadlock)");
        }
        let _ = std::fs::remove_dir_all(&stt_dir);
        let _ = std::fs::remove_dir_all(&lid_dir);
    }
}
