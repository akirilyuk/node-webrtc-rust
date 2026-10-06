//! TTS sentence-level scheduling: a long reply must not hold the TTS permit and engine
//! for its whole text.
//!
//! Needs Piper/VITS weights, so both tests are `#[ignore]` for default `cargo test`. CI runs
//! them via `bash scripts/ci/run-sherpa-example-ci.sh rust|e2e` after model download.
//!
//! Both tests set `SHERPA_POOL_MAX_CONCURRENT_TTS=1` before the global pool exists, so the
//! permit is the only thing a second request can queue on. Run with `--test-threads=1`
//! (process-wide pool, env, and `tts_generate_count`).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::{TtsProgressiveSink, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{
    create_offline_tts, reset_tts_generate_count, tts_generate_count, SherpaFactory,
};
use sherpa_onnx::GenerationConfig;
use tokio::sync::mpsc;

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

/// Six sentences of about twelve words each.
const LONG_TEXT: &str =
    "The morning train leaves the central station at a quarter past seven today. \
The conductor checks every ticket carefully before the doors close behind the last passenger. \
A small cafe near the platform sells warm bread and strong coffee to tired travellers. \
Outside the window the river runs slowly under old stone bridges covered in green moss. \
Most people read quietly or watch the fields pass while the carriages sway gently along. \
By the time the sun is fully up the train reaches the first town on the line.";

const SHORT_TEXT: &str = "Short reply now.";

/// Chunks of one sentence arrive in a burst; a new burst starts after this gap.
const BURST_GAP: Duration = Duration::from_millis(50);

fn configure_env() {
    // Set before the first `SherpaModelPool::global()` use in this process.
    unsafe {
        std::env::set_var("SHERPA_POOL_MAX_CONCURRENT_TTS", "1");
        std::env::set_var("SHERPA_TTS_PHRASE_CACHE", "0");
    }
}

fn provider(model_path: &str, project: &str) -> Arc<dyn TtsProvider> {
    let tts: Arc<dyn TtsProvider> = Arc::from(
        SherpaFactory
            .create_tts(&tts_config(model_path.to_string()))
            .expect("create TTS"),
    );
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some(project.into()),
        ..Default::default()
    });
    tts
}

/// A sink whose receiver records the arrival time of every chunk.
fn recording_sink() -> (TtsProgressiveSink, tokio::task::JoinHandle<Vec<Instant>>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let recv = tokio::spawn(async move {
        let mut arrivals = Vec::new();
        while rx.recv().await.is_some() {
            arrivals.push(Instant::now());
        }
        arrivals
    });
    (sink, recv)
}

/// Start time of every burst of chunks.
fn burst_starts(arrivals: &[Instant]) -> Vec<Instant> {
    let mut starts = Vec::new();
    let mut previous: Option<Instant> = None;
    for &at in arrivals {
        if previous.is_none_or(|prev| at.duration_since(prev) > BURST_GAP) {
            starts.push(at);
        }
        previous = Some(at);
    }
    starts
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn short_reply_interleaves_between_sentences_of_a_long_one() {
    configure_env();
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let long_tts = provider(&model_path, "sched-long");
    let short_tts = provider(&model_path, "sched-short");

    // Load the engine and warm both providers so neither request pays model load while
    // the other holds the permit.
    long_tts.synthesize("Warm up.").await.expect("warm long");
    short_tts.synthesize("Warm up.").await.expect("warm short");

    for round in 0..5 {
        reset_tts_generate_count();

        let (long_sink, long_recv) = recording_sink();
        let long_task = {
            let tts = Arc::clone(&long_tts);
            tokio::spawn(async move {
                let result = tts.synthesize_progressive(LONG_TEXT, Some(long_sink)).await;
                (Instant::now(), result)
            })
        };

        // The long request counts itself right after it takes the permit for sentence 1.
        // From here the short request queues behind it on the FIFO permit.
        let wait_start = Instant::now();
        while tts_generate_count() == 0 {
            assert!(
                wait_start.elapsed() < Duration::from_secs(30),
                "round {round}: long request never started"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let (short_sink, short_recv) = recording_sink();
        let short_task = {
            let tts = Arc::clone(&short_tts);
            tokio::spawn(async move {
                let result = tts
                    .synthesize_progressive(SHORT_TEXT, Some(short_sink))
                    .await;
                (Instant::now(), result)
            })
        };

        let (short_done, short_result) = short_task.await.expect("short join");
        let (long_done, long_result) = long_task.await.expect("long join");
        let long_arrivals = long_recv.await.expect("long recv");
        let short_arrivals = short_recv.await.expect("short recv");

        assert!(
            !short_result.expect("short synth").is_empty(),
            "round {round}: short reply produced no audio"
        );
        assert!(
            !long_result.expect("long synth").is_empty(),
            "round {round}: long reply produced no audio"
        );
        assert!(
            !short_arrivals.is_empty(),
            "round {round}: short sink got no chunks"
        );

        let bursts = burst_starts(&long_arrivals);
        eprintln!(
            "round {round}: long bursts={} short_done_after_first_long_burst={:?} \
             short_done_before_second_burst={:?} long_done_after_short={:?}",
            bursts.len(),
            short_done.duration_since(bursts[0]),
            bursts
                .get(1)
                .map(|second| second.duration_since(short_done)),
            long_done.duration_since(short_done),
        );
        assert!(
            bursts.len() >= 2,
            "round {round}: expected the long reply in several bursts, got {}",
            bursts.len()
        );
        // The short reply returned before the long reply's 2nd sentence produced audio.
        assert!(
            short_done < bursts[1],
            "round {round}: short reply finished {:?} after the long reply's 2nd sentence started \
             streaming (it waited for the whole long text)",
            short_done.duration_since(bursts[1])
        );
        assert!(
            short_done < long_done,
            "round {round}: short reply must finish before the long one"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn per_sentence_output_matches_single_call_length() {
    configure_env();
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let config = tts_config(model_path.clone());

    // Per-sentence path through the provider (phrase cache off).
    let tts = provider(&model_path, "sched-length");
    let chunks = tts.synthesize(LONG_TEXT).await.expect("provider synth");
    assert_eq!(chunks.len(), 1, "provider returns one clip");
    let provider_ms = u64::from(chunks[0].duration_ms);

    // One engine call over the whole text.
    let engine = create_offline_tts(&config).expect("create OfflineTts");
    let audio = engine
        .generate_with_config(
            LONG_TEXT,
            &GenerationConfig {
                sid: 0,
                speed: 1.0,
                ..Default::default()
            },
            None::<fn(&[f32], f32) -> bool>,
        )
        .expect("single-call generation");
    let single_ms = audio.samples().len() as u64 * 1000 / audio.sample_rate().max(1) as u64;

    eprintln!("per-sentence provider_ms={provider_ms} single_call_ms={single_ms}");
    assert!(single_ms > 0 && provider_ms > 0);
    // Piper samples noise per call, so lengths differ slightly run to run.
    let diff = provider_ms.abs_diff(single_ms) as f64;
    assert!(
        diff <= single_ms as f64 * 0.15,
        "per-sentence duration {provider_ms} ms vs single-call {single_ms} ms differs by more than 15%"
    );
}
