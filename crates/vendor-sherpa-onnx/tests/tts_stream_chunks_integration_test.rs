//! Real-model Sherpa TTS streaming (`VOICE_TTS_STREAM_CHUNKS`) integration.
//!
//! Kept `#[ignore]` for default `cargo test` (needs Piper/VITS weights). CI runs via
//! `bash scripts/ci/run-sherpa-example-ci.sh rust|e2e` after model download.
//! Use `--test-threads=1` (shared OfflineTts pool / env mutations).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::{TtsProgressiveSink, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::sync::mpsc;

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
    }
}

fn long_phrase() -> &'static str {
    // Long enough that ONNX generate wall time exceeds first progress callback on Piper low.
    "Why do you need so much time until you start speaking clearly to the listener. \
     Please explain every step carefully so that anyone following along can understand \
     the complete answer without needing to ask for clarification again."
}

async fn measure_progressive_first_chunk_ms(tts: &dyn TtsProvider, phrase: &str) -> (u128, u128, usize) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::clone(&cancel),
    };

    let progressive_count = Arc::new(AtomicUsize::new(0));
    let count_w = Arc::clone(&progressive_count);
    let first_ms = Arc::new(Mutex::new(None::<u128>));
    let first_w = Arc::clone(&first_ms);
    let started = Instant::now();

    let recv = tokio::spawn(async move {
        while let Some(_chunk) = rx.recv().await {
            let n = count_w.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                *first_w.lock().unwrap() = Some(started.elapsed().as_millis());
            }
        }
    });

    let t0 = Instant::now();
    let chunks = tts
        .synthesize_progressive(phrase, Some(sink))
        .await
        .expect("progressive synthesis");
    let full_ms = t0.elapsed().as_millis();
    let _ = recv.await;

    assert!(!chunks.is_empty(), "expected full utterance chunks");
    let first = first_ms
        .lock()
        .unwrap()
        .expect("expected at least one progressive chunk");
    let streamed = progressive_count.load(Ordering::SeqCst);
    (first, full_ms, streamed)
}

async fn measure_buffered_full_ms(tts: &dyn TtsProvider, phrase: &str) -> u128 {
    let t0 = Instant::now();
    let chunks = tts.synthesize(phrase).await.expect("buffered synthesis");
    assert!(!chunks.is_empty());
    t0.elapsed().as_millis()
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn progressive_emits_chunks_before_full_utterance() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_off = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "0");
    let _stream_on = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");

    let tts = SherpaFactory
        .create_tts(&tts_config(model_path))
        .expect("create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-stream".into()),
        ..Default::default()
    });

    let (first_ms, full_ms, streamed) =
        measure_progressive_first_chunk_ms(tts.as_ref(), long_phrase()).await;

    assert!(streamed >= 1, "expected progressive chunks, got {streamed}");
    assert!(
        first_ms + 20 <= full_ms,
        "first progressive chunk ({first_ms}ms) should arrive before full synth ({full_ms}ms)"
    );
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn progressive_first_chunk_faster_or_equal_vs_buffered_full() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_off = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "0");

    let tts = SherpaFactory
        .create_tts(&tts_config(model_path))
        .expect("create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-stream-cmp".into()),
        ..Default::default()
    });

    // Warm the engine once so cold-start does not dominate either path.
    let _ = tts.synthesize("Warm up.").await;

    let buffered_ms = measure_buffered_full_ms(tts.as_ref(), long_phrase()).await;
    let (first_ms, _full_ms, streamed) =
        measure_progressive_first_chunk_ms(tts.as_ref(), long_phrase()).await;

    assert!(streamed >= 1);
    // Allow small timer / scheduler jitter across two separate synth runs.
    assert!(
        first_ms <= buffered_ms + 100,
        "progressive first chunk ({first_ms}ms) must not be much worse than buffered full ({buffered_ms}ms)"
    );
    // When full synth is slow enough for progress callbacks to matter, require a clear win.
    if buffered_ms >= 250 {
        assert!(
            first_ms + 30 <= buffered_ms,
            "progressive first chunk ({first_ms}ms) should beat buffered full synth ({buffered_ms}ms)"
        );
    }
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn parallel_progressive_synth_completes_both_jobs() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_off = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "0");
    // Default pool allows 2 concurrent TTS — both should finish.
    let _pool = EnvGuard::set("SHERPA_POOL_MAX_CONCURRENT_TTS", "2");

    let tts_a = SherpaFactory
        .create_tts(&tts_config(model_path.clone()))
        .expect("create TTS A");
    let tts_b = SherpaFactory
        .create_tts(&tts_config(model_path))
        .expect("create TTS B");
    for (tts, project) in [
        (tts_a.as_ref(), "proj-par-a"),
        (tts_b.as_ref(), "proj-par-b"),
    ] {
        tts.bind_session_context(&VoiceSessionContext {
            project_id: Some(project.into()),
            ..Default::default()
        });
    }

    let t0 = Instant::now();
    let (a, b) = tokio::join!(
        tts_a.synthesize_progressive("Hello from client A.", None),
        tts_b.synthesize_progressive("Hello from client B.", None),
    );
    let wall_ms = t0.elapsed().as_millis();

    let chunks_a = a.expect("client A synth");
    let chunks_b = b.expect("client B synth");
    assert!(!chunks_a.is_empty());
    assert!(!chunks_b.is_empty());
    // With pool=2, wall time should stay well under serial 2x of a short phrase (~many seconds).
    assert!(
        wall_ms < 60_000,
        "parallel short phrases should finish within 60s, took {wall_ms}ms"
    );
}


#[tokio::test]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn progressive_pcm_matches_final_oneshot() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _cache_off = EnvGuard::set("SHERPA_TTS_PHRASE_CACHE", "0");

    let tts = SherpaFactory
        .create_tts(&tts_config(model_path))
        .expect("create TTS");
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("proj-pcm-cmp".into()),
        ..Default::default()
    });

    let phrase = long_phrase();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::clone(&cancel),
    };

    let recv = tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut n = 0usize;
        while let Some(chunk) = rx.recv().await {
            n += 1;
            bytes.extend_from_slice(&chunk.pcm);
        }
        (bytes, n)
    });

    let full = tts
        .synthesize_progressive(phrase, Some(sink))
        .await
        .expect("progressive");
    let (streamed, n) = recv.await.expect("join");
    // Same generate() result — padded to 20 ms (return path); streamed has no pad.
    let oneshot = &full[0].pcm;

    eprintln!(
        "progressive chunks={n} streamed_bytes={} oneshot_bytes={}",
        streamed.len(),
        oneshot.len()
    );

    assert!(n >= 1, "expected progressive chunks");
    let diff = (streamed.len() as i64 - oneshot.len() as i64).abs();
    assert!(
        diff <= 3840,
        "progressive streamed PCM should nearly match oneshot (diff={diff}, streamed={}, oneshot={})",
        streamed.len(),
        oneshot.len()
    );

    let ncmp = streamed.len().min(oneshot.len());
    let mism = streamed[..ncmp]
        .iter()
        .zip(oneshot[..ncmp].iter())
        .filter(|(a, b)| a != b)
        .count();
    eprintln!("mismatched bytes in prefix {ncmp}: {mism}");
    assert_eq!(mism, 0, "streamed progressive PCM must match oneshot convert");
}
