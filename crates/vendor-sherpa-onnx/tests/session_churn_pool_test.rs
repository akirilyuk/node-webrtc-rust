//! Session churn against the real Sherpa pool: `active_sessions` must return to zero, the pool
//! must not grow, and (Linux) the process thread count must not climb.
//!
//! `#[ignore]` for default `cargo test` (needs STT + TTS weights). CI runs it via
//! `scripts/ci/run-sherpa-example-ci.sh` after `ensure_sherpa_models`. Run with `--test-threads=1`.

use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::{SherpaFactory, SherpaModelPool};

const CYCLES: usize = 100;
const CHUNK_BYTES: usize = 1920;

fn stt_config(model_path: String) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        language: Some("en".into()),
        api_key: None,
        endpoint: None,
    }
}

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_keep_alive(Duration::from_millis(100))
        .enable_all()
        .build()
        .expect("tokio runtime")
}

#[cfg(target_os = "linux")]
fn process_thread_count() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|rest| rest.trim().parse::<usize>().ok())
        .expect("Threads: line in /proc/self/status")
}

#[test]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
fn stt_churn_returns_active_sessions_to_zero() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let cfg = stt_config(stt_path);
    let rt = runtime();

    let mut entries_after_first: Option<usize> = None;
    #[cfg(target_os = "linux")]
    let mut threads_after_cycle_10: Option<usize> = None;

    for cycle in 1..=CYCLES {
        rt.block_on(async {
            let factory = SherpaFactory;
            let mut stt = factory.create_stt(&cfg).expect("create_stt");
            stt.start().await.expect("stt start");
            let silence = vec![0_u8; 32_000];
            for chunk in silence.chunks(CHUNK_BYTES) {
                stt.push_audio(Bytes::copy_from_slice(chunk))
                    .await
                    .expect("push_audio");
                while stt.poll_transcript().await.expect("poll").is_some() {}
            }
            stt.stop().await.expect("stt stop");
            drop(stt);
        });
        if cycle == 1 {
            entries_after_first = Some(SherpaModelPool::global().stt_entry_count());
        }
        #[cfg(target_os = "linux")]
        if cycle == 10 {
            threads_after_cycle_10 = Some(process_thread_count());
        }
    }

    let pool = SherpaModelPool::global();
    assert_eq!(
        Some(pool.stt_entry_count()),
        entries_after_first,
        "stt pool entry count changed over {CYCLES} cycles"
    );
    let shared = pool.get_or_create_stt(&cfg).expect("pooled stt");
    assert_eq!(
        shared.active_sessions(),
        0,
        "active_sessions must return to zero after {CYCLES} start/stop cycles"
    );

    #[cfg(target_os = "linux")]
    {
        std::thread::sleep(Duration::from_millis(500));
        let end = process_thread_count();
        let baseline = threads_after_cycle_10.expect("thread count after cycle 10");
        assert!(
            end <= baseline,
            "process thread count grew: after cycle 10 = {baseline}, at end = {end}"
        );
    }
}

#[test]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
fn tts_churn_returns_active_sessions_to_zero() {
    let tts_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let cfg = tts_config(tts_path);
    let rt = runtime();

    let factory = SherpaFactory;
    let tts = factory.create_tts(&cfg).expect("create_tts");
    for i in 0..CYCLES {
        // Distinct text per call so the phrase cache cannot short-circuit synthesis.
        let chunks = rt
            .block_on(tts.synthesize(&format!("churn number {i}")))
            .unwrap_or_else(|e| panic!("synthesize {i}: {e}"));
        assert!(!chunks.is_empty(), "synthesize {i} returned no chunks");
    }

    let pool = SherpaModelPool::global()
        .get_or_create_tts(&cfg)
        .expect("pooled tts");
    for slot in 0..pool.len() {
        assert_eq!(
            pool.acquire().active_sessions(),
            0,
            "tts engine slot {slot} still has active sessions after {CYCLES} calls"
        );
    }
}
