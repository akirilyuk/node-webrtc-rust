//! B3: every chunk delivered to a progressive sink must stay small enough for one gRPC message.
//!
//! One long sentence without punctuation is a single Piper progress callback on first play, and a
//! phrase-cache hit (`CachingTtsProvider` in the speech crate) delivers the whole utterance at once. Both exceed tonic's 4 MiB default once
//! the audio is longer than ~21.8 s of stereo 48 kHz s16le. Each sink chunk must stay at or under
//! one second of audio.
//!
//! Kept `#[ignore]` for default `cargo test` (needs Piper/VITS weights). CI runs it via
//! `bash scripts/ci/run-sherpa-example-ci.sh rust|e2e` after model download.
//! Run with `--test-threads=1` (process-wide env + OfflineTts pool).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor, VoiceSessionContext};
use node_webrtc_rust_speech::pipeline::{TtsProgressiveSink, TtsProvider, VendorFactory};
use node_webrtc_rust_speech::tts_cache::{CachingTtsProvider, PhraseCache};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;
use tokio::sync::mpsc;

/// 1 s of stereo 48 kHz s16le: 48_000 frames x 2 channels x 2 bytes.
const MAX_SINK_CHUNK_BYTES: usize = 192_000;

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
        endpoint: None,
    }
}

/// 90 words, no punctuation: one Piper sentence.
fn ninety_words_no_punctuation() -> String {
    let words: Vec<&str> = "the quick brown fox jumps over the lazy dog"
        .split(' ')
        .collect();
    (0..90)
        .map(|i| words[i % words.len()])
        .collect::<Vec<_>>()
        .join(" ")
}

fn sink_pair() -> (
    TtsProgressiveSink,
    mpsc::UnboundedReceiver<node_webrtc_rust_speech::pipeline::TtsAudioChunk>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        TtsProgressiveSink {
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
        },
        rx,
    )
}

fn drain_lengths(
    rx: &mut mpsc::UnboundedReceiver<node_webrtc_rust_speech::pipeline::TtsAudioChunk>,
) -> Vec<usize> {
    let mut lengths = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        lengths.push(chunk.pcm.len());
    }
    lengths
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_TTS_MODEL_PATH with valid Piper/VITS bundle"]
async fn sink_chunks_never_exceed_one_second() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let _stream_on = EnvGuard::set("VOICE_TTS_STREAM_CHUNKS", "1");

    let config = tts_config(model_path);
    let inner = SherpaFactory.create_tts(&config).expect("create TTS");
    let tts = CachingTtsProvider::new(
        inner,
        config,
        Arc::new(PhraseCache::new(64 * 1024 * 1024, 32 * 1024 * 1024)),
    );
    tts.bind_session_context(&VoiceSessionContext {
        project_id: Some("p-b3-sink".into()),
        ..Default::default()
    });
    let text = ninety_words_no_punctuation();
    assert_eq!(text.split_whitespace().count(), 90);

    // Call 1: fresh provider, cache miss, progressive path.
    let (sink, mut rx) = sink_pair();
    tts.synthesize_progressive(&text, Some(sink))
        .await
        .expect("first progressive synth");
    let first = drain_lengths(&mut rx);
    println!("first play: {} sink chunks, lengths={first:?}", first.len());
    assert!(!first.is_empty(), "call 1: sink received no chunks");
    let first_max = first.iter().copied().max().unwrap_or(0);

    // Call 2: same text, same provider: phrase-cache hit path.
    let (sink, mut rx) = sink_pair();
    tts.synthesize_progressive(&text, Some(sink))
        .await
        .expect("second progressive synth");
    let second = drain_lengths(&mut rx);
    println!("replay: {} sink chunks, lengths={second:?}", second.len());
    assert!(!second.is_empty(), "call 2: sink received no chunks");
    let second_max = second.iter().copied().max().unwrap_or(0);

    assert!(
        first_max <= MAX_SINK_CHUNK_BYTES,
        "call 1 (first play): a sink chunk has {first_max} bytes, limit {MAX_SINK_CHUNK_BYTES}"
    );
    assert!(
        second_max <= MAX_SINK_CHUNK_BYTES,
        "call 2 (cache hit): a sink chunk has {second_max} bytes, limit {MAX_SINK_CHUNK_BYTES}"
    );
}
