//! Speech engine cost probes: TTS cost per voice and STT streams per CPU.
//!
//! Both probes are `#[ignore]` (they need downloaded models) and print one
//! `perf_probe {json}` line per measured point, the shape `scripts/perf/collect-perf.mjs` parses
//! and `scripts/perf/voice-cost-report.mjs` aggregates. Run them through
//! `scripts/perf/run-voice-cost.sh`, which pins them to a CPU-limited container; run natively with
//!
//! ```text
//! SHERPA_TTS_MODEL_PATH=<voice dir> cargo test --release -p node-webrtc-rust-vendor-sherpa-onnx \
//!   --test voice_cost_probe tts_voice_cost_probe -- --ignored --nocapture --test-threads=1
//! SHERPA_STT_MODEL_PATH=<stt dir> cargo test --release -p node-webrtc-rust-vendor-sherpa-onnx \
//!   --test voice_cost_probe stt_stream_capacity_probe -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `tts_voice_cost_probe`
//!   - `SHERPA_TTS_MODEL_PATH` (required): one voice bundle directory.
//!   - `PROBE_TTS_SESSIONS` (default `1,2,4`): concurrent syntheses per measured point.
//!   - `PROBE_LONG_REPS` (default `3`): repetitions of the ~400 character text (M = 1 only).
//!   - `SHERPA_POOL_MAX_CONCURRENT_TTS`: when unset the probe raises the pool limit to the largest
//!     M, so M really means M syntheses running at once instead of queueing behind the default 2.
//!
//!   Per M: 5 rounds of M concurrent short syntheses give time-to-first-audio p50/p95. At M = 1 the
//!   long text gives the real-time factor (wall seconds per audio second). CPU seconds per audio
//!   second covers every synthesis measured at that M (process CPU time from `getrusage`).
//!
//! `stt_stream_capacity_probe`
//!   - `SHERPA_STT_MODEL_PATH` (required): a streaming recognizer bundle directory.
//!   - `PROBE_STT_STREAMS` (default `1,4,8,16,24,32,40,48`): concurrent streams per point.
//!   - `PROBE_STT_LAG_SLO_MS` (default `500`): the point is real-time while the p95 over streams
//!     of the worst per-chunk decode lag stays at or below this.
//!
//!   N streams each feed a 10 s clip in real time (20 ms chunks on an absolute clock). Lag is how
//!   late a chunk's push-and-poll returns after the chunk was due; a decoder that cannot keep up
//!   makes it grow. Final time is the span from the last chunk's due time to the drained final.
//!   The probe stops after the first N that is not real-time.
//!
//! Peak RSS is the process maximum so far, so it only grows across the points of one run.
//! Use `--test-threads=1`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{
    SttProvider, TtsProgressiveSink, TtsProvider, VendorFactory,
};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::sync::{mpsc, Barrier};

const SHORT_TEXT: &str = "Hello, thanks for calling. How can I help you today?";
const LONG_TEXT: &str = "Thanks for holding. I looked at your account and found two open \
    orders. The first one shipped on Monday and should arrive by Thursday afternoon, while the \
    second is still waiting for a part that the warehouse expects next week. I can send a \
    tracking link to your phone, change the delivery address before the second parcel leaves, \
    or combine both orders into one shipment. Tell me which option works best for you.";
const SHORT_ROUNDS: usize = 5;
/// 48 kHz stereo s16le.
const TTS_BYTES_PER_SECOND: f64 = 48_000.0 * 2.0 * 2.0;
/// 20 ms of 16 kHz mono.
const STT_CHUNK_SAMPLES: usize = 320;
const STT_CHUNK_MS: u64 = 20;
const STT_CLIP_SAMPLES: usize = 160_000;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|&value| value > 0)
        .unwrap_or(default)
}

fn env_usize_list(name: &str, default: &str) -> Vec<usize> {
    let raw = std::env::var(name).unwrap_or_else(|_| default.to_string());
    let list: Vec<usize> = raw
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .filter(|&value| value > 0)
        .collect();
    assert!(!list.is_empty(), "{name} must list positive integers");
    list
}

fn model_name(model_path: &str) -> String {
    Path::new(model_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| model_path.to_string())
}

/// Nearest-rank percentile of `values` (`pct` in 0..=100).
fn percentile(values: &[f64], pct: f64) -> f64 {
    assert!(!values.is_empty(), "percentile of no samples");
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite sample"));
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

struct Usage {
    cpu_s: f64,
    rss_mb_peak: f64,
}

fn usage() -> Usage {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    assert_eq!(rc, 0, "getrusage failed");
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    // ru_maxrss is bytes on macOS and KiB on Linux.
    #[cfg(target_os = "macos")]
    let rss_mb_peak = ru.ru_maxrss as f64 / (1024.0 * 1024.0);
    #[cfg(not(target_os = "macos"))]
    let rss_mb_peak = ru.ru_maxrss as f64 / 1024.0;
    Usage {
        cpu_s: tv(ru.ru_utime) + tv(ru.ru_stime),
        rss_mb_peak,
    }
}

fn emit(name: &str, params: &str, metrics: &str) {
    println!(
        "perf_probe {{\"name\":\"{name}\",\"params\":{{{params}}},\"metrics\":{{{metrics}}}}}"
    );
}

// ---------------------------------------------------------------- TTS

fn tts_config(model_path: &str) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path.to_string()),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
    }
}

struct Synthesis {
    ttfa_ms: f64,
    wall_s: f64,
    audio_s: f64,
}

/// One synthesis of `text` per provider, all started together.
async fn synth_round(providers: &[Arc<dyn TtsProvider>], text: &'static str) -> Vec<Synthesis> {
    let barrier = Arc::new(Barrier::new(providers.len()));
    let mut handles = Vec::new();
    for tts in providers {
        let tts = Arc::clone(tts);
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let sink = TtsProgressiveSink {
                tx,
                cancel: Arc::new(AtomicBool::new(false)),
            };
            barrier.wait().await;
            let started = Instant::now();
            let synth = tts.synthesize_progressive(text, Some(sink));
            let first_chunk = async {
                let mut first = None;
                while rx.recv().await.is_some() {
                    first.get_or_insert_with(|| started.elapsed());
                }
                first
            };
            let (chunks, first) = tokio::join!(synth, first_chunk);
            let wall = started.elapsed();
            let chunks = chunks.expect("synthesis");
            let bytes: usize = chunks.iter().map(|chunk| chunk.pcm.len()).sum();
            assert!(bytes > 0, "synthesis produced no audio");
            Synthesis {
                ttfa_ms: first.unwrap_or(wall).as_secs_f64() * 1000.0,
                wall_s: wall.as_secs_f64(),
                audio_s: bytes as f64 / TTS_BYTES_PER_SECOND,
            }
        }));
    }
    let mut out = Vec::new();
    for handle in handles {
        out.push(handle.await.expect("synthesis task"));
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with a valid TTS bundle"]
async fn tts_voice_cost_probe() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let sessions = env_usize_list("PROBE_TTS_SESSIONS", "1,2,4");
    let long_reps = env_usize("PROBE_LONG_REPS", 3);
    let threads = env_usize("SHERPA_TTS_NUM_THREADS", 2);
    let max_sessions = *sessions.iter().max().expect("sessions");
    if std::env::var_os("SHERPA_POOL_MAX_CONCURRENT_TTS").is_none() {
        // Read once when the shared pool is created, so set it before the first provider.
        std::env::set_var("SHERPA_POOL_MAX_CONCURRENT_TTS", max_sessions.to_string());
    }
    let permits = env_usize("SHERPA_POOL_MAX_CONCURRENT_TTS", 2);
    let model = model_name(&model_path);

    for &m in &sessions {
        let providers: Vec<Arc<dyn TtsProvider>> = (0..m)
            .map(|_| {
                Arc::from(
                    SherpaFactory
                        .create_tts(&tts_config(&model_path))
                        .expect("create TTS"),
                )
            })
            .collect();

        // Warm-up: model load, engine creation, first-call allocations.
        synth_round(&providers, SHORT_TEXT).await;

        let before = usage();
        let mut ttfa = Vec::new();
        let mut audio_s = 0.0;
        for _ in 0..SHORT_ROUNDS {
            for synth in synth_round(&providers, SHORT_TEXT).await {
                ttfa.push(synth.ttfa_ms);
                audio_s += synth.audio_s;
            }
        }
        let mut rtf = Vec::new();
        if m == 1 {
            for _ in 0..long_reps {
                for synth in synth_round(&providers, LONG_TEXT).await {
                    rtf.push(synth.wall_s / synth.audio_s);
                    audio_s += synth.audio_s;
                }
            }
        }
        let after = usage();

        let mut metrics = format!(
            "\"tts_short_ttfa_ms_p50\":{:.1},\"tts_short_ttfa_ms_p95\":{:.1}",
            percentile(&ttfa, 50.0),
            percentile(&ttfa, 95.0)
        );
        if !rtf.is_empty() {
            metrics.push_str(&format!(
                ",\"tts_long_rtf_p50\":{:.4}",
                percentile(&rtf, 50.0)
            ));
        }
        metrics.push_str(&format!(
            ",\"tts_cpu_s_per_audio_s\":{:.4},\"rss_mb_peak\":{:.1}",
            (after.cpu_s - before.cpu_s) / audio_s,
            after.rss_mb_peak
        ));
        emit(
            "tts_voice_cost_probe",
            &format!(
                "\"model\":\"{model}\",\"sessions\":{m},\"tts_num_threads\":{threads},\"tts_permits\":{permits}"
            ),
            &metrics,
        );
    }
}

// ---------------------------------------------------------------- STT

fn stt_config(model_path: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path.to_string()),
        language: Some("en".into()),
        api_key: None,
        endpoint: None,
    }
}

fn clip_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rx-echo-onset-s5.wav")
}

/// 16 kHz mono s16le speech looped to 10 s, cut into 20 ms chunks.
fn clip_chunks() -> Vec<Bytes> {
    let data = std::fs::read(clip_path()).expect("read fixture wav");
    assert!(data.len() > 44, "wav too short");
    let samples: Vec<i16> = data[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    assert!(!samples.is_empty(), "fixture has no samples");
    let looped: Vec<i16> = samples
        .iter()
        .copied()
        .cycle()
        .take(STT_CLIP_SAMPLES)
        .collect();
    looped
        .chunks_exact(STT_CHUNK_SAMPLES)
        .map(|chunk| {
            Bytes::from(
                chunk
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect::<Vec<u8>>(),
            )
        })
        .collect()
}

async fn drain(stt: &mut Box<dyn SttProvider>) {
    while stt.poll_transcript().await.expect("poll").is_some() {}
}

/// Returns (max lag ms, final ms) for one stream fed in real time.
async fn run_stream(
    mut stt: Box<dyn SttProvider>,
    chunks: Arc<Vec<Bytes>>,
    ready: Arc<Barrier>,
) -> (f64, f64) {
    stt.start().await.expect("stt start");
    assert!(
        stt.wait_ready(Duration::from_secs(30))
            .await
            .expect("ready"),
        "stt stream not ready"
    );
    ready.wait().await;
    let t0 = Instant::now();
    let chunk = Duration::from_millis(STT_CHUNK_MS);
    let mut max_lag = Duration::ZERO;
    let mut out = Vec::new();
    let mut last_due = t0;
    for (index, pcm) in chunks.iter().enumerate() {
        let due = t0 + chunk * (index as u32 + 1);
        tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await;
        out.clear();
        stt.push_and_poll(pcm.clone(), &mut out)
            .await
            .expect("push_and_poll");
        max_lag = max_lag.max(Instant::now().saturating_duration_since(due));
        last_due = due;
    }
    stt.finalize_utterance().await.expect("finalize");
    drain(&mut stt).await;
    let final_ms = Instant::now().saturating_duration_since(last_due);
    stt.stop().await.expect("stt stop");
    (
        max_lag.as_secs_f64() * 1000.0,
        final_ms.as_secs_f64() * 1000.0,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_STT_MODEL_PATH with a valid streaming bundle"]
async fn stt_stream_capacity_probe() {
    let model_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let streams = env_usize_list("PROBE_STT_STREAMS", "1,4,8,16,24,32,40,48");
    let slo_ms = env_usize("PROBE_STT_LAG_SLO_MS", 500) as f64;
    let threads = env_usize("SHERPA_STT_NUM_THREADS", 1);
    let model = model_name(&model_path);
    let cfg = stt_config(&model_path);
    let chunks = Arc::new(clip_chunks());
    let audio_s_per_stream = chunks.len() as f64 * STT_CHUNK_MS as f64 / 1000.0;

    // Warm-up: recognizer creation, first decode.
    {
        let warm = SherpaFactory.create_stt(&cfg).expect("create STT");
        let head: Arc<Vec<Bytes>> = Arc::new(chunks.iter().take(100).cloned().collect());
        run_stream(warm, head, Arc::new(Barrier::new(1))).await;
    }

    for &n in &streams {
        let ready = Arc::new(Barrier::new(n + 1));
        let handles: Vec<_> = (0..n)
            .map(|_| {
                let stt = SherpaFactory.create_stt(&cfg).expect("create STT");
                tokio::spawn(run_stream(stt, Arc::clone(&chunks), Arc::clone(&ready)))
            })
            .collect();
        ready.wait().await;
        let before = usage();
        let mut lags = Vec::new();
        let mut finals = Vec::new();
        for handle in handles {
            let (lag_ms, final_ms) = handle.await.expect("stream task");
            lags.push(lag_ms);
            finals.push(final_ms);
        }
        let after = usage();

        let lag_p95 = percentile(&lags, 95.0);
        let realtime_ok = lag_p95 <= slo_ms;
        emit(
            "stt_stream_capacity_probe",
            &format!("\"model\":\"{model}\",\"streams\":{n},\"stt_num_threads\":{threads}"),
            &format!(
                "\"stt_lag_ms_p95\":{:.1},\"stt_final_ms_p95\":{:.1},\"stt_realtime_ok\":{realtime_ok},\"cpu_s_per_audio_s\":{:.4},\"rss_mb_peak\":{:.1}",
                lag_p95,
                percentile(&finals, 95.0),
                (after.cpu_s - before.cpu_s) / (n as f64 * audio_s_per_stream),
                after.rss_mb_peak
            ),
        );
        if !realtime_ok {
            break;
        }
    }
}
