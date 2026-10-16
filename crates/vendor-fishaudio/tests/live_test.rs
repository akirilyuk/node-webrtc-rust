//! Live Drama 3 checks against the real Fish Audio API. Run with `--features live`.
//!
//! Needs `FISH_API_KEY` (CI maps the repo secret `FISH_AUDIO_API` to it). Without a key the
//! tests print a note and return, unless `FISH_LIVE_REQUIRED=1`, which turns a missing key
//! into a panic so CI can never go green on a skipped run. The key is never printed.
//! Optional `FISH_VOICE_ID` picks a `reference_id` for the first two tests.
//!
//! Run with `--test-threads=1`: the Starter tier allows 5 concurrent requests and the tests
//! print timing lines (`fishaudio_*`) that CI copies into the step summary.
#![cfg(feature = "live")]

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use node_webrtc_rust_speech::pipeline::{TtsAudioChunk, TtsProgressiveSink};
use node_webrtc_rust_vendor_fishaudio::__test::Client;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const MODEL: &str = "drama-3";
const TEXT: &str =
    "[warm, friendly] Hello from Drama three. <emphasis>This</emphasis> is a streaming check.";
/// 48 kHz stereo s16le: 4 bytes per frame, 192 bytes per millisecond.
const BYTES_PER_MS: usize = 192;

fn live_key() -> Option<String> {
    let key = std::env::var("FISH_API_KEY").unwrap_or_default();
    if key.is_empty() {
        if std::env::var("FISH_LIVE_REQUIRED").as_deref() == Ok("1") {
            panic!("FISH_LIVE_REQUIRED=1 but FISH_API_KEY is empty");
        }
        println!("fishaudio live test skipped: FISH_API_KEY unset");
        return None;
    }
    Some(key)
}

fn env_voice() -> Option<String> {
    std::env::var("FISH_VOICE_ID")
        .ok()
        .filter(|voice| !voice.is_empty())
}

fn client(key: String) -> Client {
    Client::new(Some(key), None).unwrap_or_else(|err| panic!("client: {err}"))
}

fn duration_ms(pcm_len: usize) -> usize {
    pcm_len / BYTES_PER_MS
}

fn rms(pcm: &[u8]) -> f64 {
    let samples: Vec<i16> = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    assert!(!samples.is_empty(), "no PCM samples");
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt()
}

struct SinkStats {
    chunks: usize,
    bytes: usize,
    first_at: Option<Instant>,
}

/// Sink plus a task that timestamps the first chunk and counts what arrives.
fn sink() -> (TtsProgressiveSink, JoinHandle<SinkStats>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<TtsAudioChunk>();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let task = tokio::spawn(async move {
        let mut stats = SinkStats {
            chunks: 0,
            bytes: 0,
            first_at: None,
        };
        while let Some(chunk) = rx.recv().await {
            stats.first_at.get_or_insert_with(Instant::now);
            stats.chunks += 1;
            stats.bytes += chunk.pcm.len();
        }
        stats
    });
    (sink, task)
}

/// Runs one transport with a sink attached and checks the streaming contract.
async fn assert_streams(transport: &str) {
    let Some(key) = live_key() else { return };
    let client = client(key);
    let voice = env_voice();
    let (sink, task) = sink();

    let started = Instant::now();
    let result = if transport == "ws" {
        client
            .synthesize_ws(TEXT, voice.as_deref(), MODEL, Some(&sink))
            .await
    } else {
        client
            .synthesize_http(TEXT, voice.as_deref(), MODEL, Some(&sink))
            .await
    };
    let returned_at = Instant::now();
    // Dropping the sink closes the channel so the receiver task finishes.
    drop(sink);
    let pcm = match result {
        Ok(pcm) => pcm,
        Err(err) => panic!("fishaudio {transport} synthesis failed: {err}"),
    };
    let stats = task.await.expect("sink task");

    let first_at = stats.first_at.expect("no sink chunk arrived");
    let ttfa_ms = first_at.duration_since(started).as_millis();
    println!("fishaudio_ttfa_ms transport={transport} {ttfa_ms}");

    assert!(
        stats.chunks >= 2,
        "expected >= 2 sink chunks, got {}",
        stats.chunks
    );
    assert!(
        first_at < returned_at,
        "first sink chunk arrived after the call returned"
    );
    let total_ms = duration_ms(pcm.len());
    assert!(
        (1500..=15000).contains(&total_ms),
        "audio duration {total_ms} ms outside 1500..=15000"
    );
    assert_eq!(
        stats.bytes,
        pcm.len(),
        "sink bytes differ from returned bytes"
    );
    let level = rms(&pcm);
    assert!(level > 200.0, "RMS {level:.1} <= 200: audio is near-silent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drama3_live_ws_streams() {
    assert_streams("ws").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drama3_live_http_streams() {
    assert_streams("http").await;
}

/// Decides whether Fish accepts a request without `reference_id` (plan §9 Q3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drama3_live_without_voice() {
    let Some(key) = live_key() else { return };
    let client = client(key);
    match client.synthesize_ws(TEXT, None, MODEL, None).await {
        Ok(pcm) => {
            let ms = duration_ms(pcm.len());
            println!("fishaudio_no_voice status=ok duration_ms={ms}");
            assert!(ms > 1000, "no-voice audio too short: {ms} ms");
        }
        Err(err) => {
            println!("fishaudio_no_voice status=error message={err}");
            panic!("synthesis without reference_id failed: {err}");
        }
    }
}
