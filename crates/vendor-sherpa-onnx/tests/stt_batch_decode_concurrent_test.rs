//! Concurrent sessions batch on the decode thread and each keeps its own results (no equality
//! between modes: batched decode can differ by a near-tie token).
//!
//! `#[ignore]` (needs STT + TTS weights). Run with `--test-threads=1`: the switch and the batch
//! counters are process-wide (own test binary, one test).

use std::sync::Arc;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{
    reset_sherpa_batch_counters, set_stt_batch_decode, sherpa_batch_rounds,
    sherpa_batch_stream_steps, SherpaFactory,
};
use tokio::sync::Barrier;

/// Every phrase has words no other phrase has (the routing assertion relies on it). The shape
/// follows the phrases of `stt_concurrent_equals_serial_test.rs`; very short ones ("golf nineteen
/// twenty") sometimes produced no final.
const PHRASES: [&str; 8] = [
    "alpha one two three",
    "bravo four five six",
    "delta seven eight nine",
    "echo ten eleven twelve",
    "foxtrot thirteen fourteen fifteen",
    "juliet sixteen seventeen eighteen",
    "kilo nineteen twenty thirty",
    "lima forty fifty sixty",
];
/// 600 ms of 16 kHz mono s16le. Big messages keep every request on the decode thread for many
/// steps, so the other sessions' requests arrive while a round runs and join the next one
/// (with 60 ms messages the sessions could drift apart and never share a round).
const CHUNK_BYTES: usize = 19200;

fn stt_config(model_path: String) -> SttConfig {
    SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        language: Some("en".into()),
        api_key: None,
        endpoint: None,
        transport: None,
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
        sentence_case: Default::default(),
        transport: None,
    }
}

async fn synthesize_speech_pcm_16k(text: &str) -> Bytes {
    let tts_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&tts_config(tts_path))
        .expect("factory should create TTS");
    let chunks = tts.synthesize(text).await.expect("piper synth");
    let mut stereo = Vec::new();
    for chunk in chunks {
        stereo.extend_from_slice(chunk.pcm.as_ref());
    }
    let mono = stereo_48k_to_mono_16k(&stereo);
    i16_samples_to_bytes(&mono)
}

fn take_finals(finals: &mut Vec<String>, transcripts: Vec<SttTranscript>) {
    for transcript in transcripts {
        if let SttTranscript::Final(text) = transcript {
            finals.push(text.trim().to_string());
        }
    }
}

/// Ordered final texts of one session fed with `push_and_poll` as fast as the calls return.
async fn run_session(cfg: &SttConfig, pcm: &[u8], barrier: Arc<Barrier>) -> Vec<String> {
    let factory = SherpaFactory;
    let mut stt = factory.create_stt(cfg).expect("create_stt");
    stt.start().await.expect("stt start");
    barrier.wait().await;
    let mut finals = Vec::new();
    for chunk in pcm.chunks(CHUNK_BYTES) {
        let mut out = Vec::new();
        stt.push_and_poll(Bytes::copy_from_slice(chunk), &mut out)
            .await
            .expect("push_and_poll");
        take_finals(&mut finals, out);
    }
    stt.finalize_utterance().await.expect("finalize_utterance");
    while let Some(transcript) = stt.poll_transcript().await.expect("poll") {
        take_finals(&mut finals, vec![transcript]);
    }
    stt.stop().await.expect("stt stop");
    finals.retain(|text| !text.is_empty());
    finals
}

async fn run_all(cfg: &SttConfig, pcms: &[Bytes]) -> Vec<Vec<String>> {
    let barrier = Arc::new(Barrier::new(pcms.len()));
    let mut handles = Vec::new();
    for pcm in pcms {
        let cfg = cfg.clone();
        let pcm = pcm.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            run_session(&cfg, pcm.as_ref(), barrier).await
        }));
    }
    let mut results = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("session task"));
    }
    results
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
async fn concurrent_sessions_batch_and_keep_their_own_results() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let cfg = stt_config(stt_path);

    let mut pcms: Vec<Bytes> = Vec::new();
    for phrase in PHRASES {
        let pcm = synthesize_speech_pcm_16k(phrase).await;
        assert!(
            pcm.len() >= 3200,
            "phrase {phrase:?} produced too little PCM"
        );
        pcms.push(pcm);
    }

    set_stt_batch_decode(false);
    reset_sherpa_batch_counters();
    let off = run_all(&cfg, &pcms).await;
    assert_eq!(sherpa_batch_rounds(), 0, "switch off must not batch");
    assert_eq!(sherpa_batch_stream_steps(), 0, "switch off must not batch");

    set_stt_batch_decode(true);
    reset_sherpa_batch_counters();
    let on = run_all(&cfg, &pcms).await;
    let rounds = sherpa_batch_rounds();
    let steps = sherpa_batch_stream_steps();
    println!("batch_counters rounds={rounds} stream_steps={steps}");

    let words = |text: &[String]| -> std::collections::HashSet<String> {
        text.iter()
            .flat_map(|t| {
                t.to_lowercase()
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    let overlap = |a: &[String], b: &[String]| words(a).intersection(&words(b)).count();
    let mut differing = 0;
    for idx in 0..PHRASES.len() {
        assert!(!off[idx].is_empty(), "session {idx} has no final (off)");
        assert!(!on[idx].is_empty(), "session {idx} has no final (on)");
        if on[idx] != off[idx] {
            differing += 1;
        }
        let own = overlap(&on[idx], &off[idx]);
        for (other, other_off) in off.iter().enumerate() {
            if other != idx {
                let cross = overlap(&on[idx], other_off);
                assert!(
                    own > cross,
                    "session {idx} on-final {:?} overlaps own off-final {own} but session {other} off-final {cross} ({:?})",
                    on[idx], other_off
                );
            }
        }
    }
    println!("finals_differing={differing}");
    assert!(rounds > 0, "switch on must run batch rounds");
    assert!(
        steps > rounds,
        "no round decoded more than one stream: rounds={rounds} stream_steps={steps}"
    );
}
