//! A `push_and_poll` call dropped mid-way must still deliver its audio to the stream exactly
//! once (the accept happens under the session guard that the decode thread then owns).
//!
//! `#[ignore]` (needs STT + TTS weights). Run with `--test-threads=1` (process-wide switch).

use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{set_stt_batch_decode, SherpaFactory};

const PHRASE: &str = "alpha one two three four five six";
/// 20 ms of 16 kHz mono s16le.
const CHUNK_BYTES: usize = 640;

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

/// Final texts of one session. A message for which `drop_it(index, total)` is true is wrapped in a
/// zero timeout so the call is dropped (possibly while it waits for the session lock, which a
/// previous dropped call's decode request still holds); the message is not resent. Transcripts of a dropped call are lost by
/// design (they are returned only to the dropped future), so only the finals after the
/// `finalize_utterance` drain are compared by the caller.
async fn run(cfg: &SttConfig, pcm: &[u8], drop_it: fn(usize, usize) -> bool) -> String {
    let factory = SherpaFactory;
    let mut stt = factory.create_stt(cfg).expect("create_stt");
    stt.start().await.expect("stt start");
    let total = pcm.chunks(CHUNK_BYTES).count();
    for (idx, chunk) in pcm.chunks(CHUNK_BYTES).enumerate() {
        let mut out = Vec::new();
        let call = stt.push_and_poll(Bytes::copy_from_slice(chunk), &mut out);
        if drop_it(idx, total) {
            let _ = tokio::time::timeout(Duration::ZERO, call).await;
        } else {
            call.await.expect("push_and_poll");
        }
    }
    stt.finalize_utterance().await.expect("finalize_utterance");
    let mut finals = Vec::new();
    while let Some(transcript) = stt.poll_transcript().await.expect("poll") {
        if let SttTranscript::Final(text) = transcript {
            finals.push(text.trim().to_string());
        }
    }
    stt.stop().await.expect("stt stop");
    finals.retain(|text| !text.is_empty());
    finals.join(" ")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
async fn dropped_call_does_not_lose_or_duplicate_audio() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let cfg = stt_config(stt_path);
    set_stt_batch_decode(true);
    let pcm = synthesize_speech_pcm_16k(PHRASE).await;
    assert!(pcm.len() >= 3200, "too little PCM");

    let total = pcm.len().div_ceil(CHUNK_BYTES);
    assert!(total >= 60, "too few messages: {total}");

    let reference = run(&cfg, pcm.as_ref(), |_, _| false).await;
    assert!(!reference.is_empty(), "undisturbed run has no final");
    // Scenario 1: every 5th call dropped.
    let every_fifth = run(&cfg, pcm.as_ref(), |idx, _| idx % 5 == 4).await;
    assert_eq!(every_fifth, reference, "every 5th call dropped");
    // Scenario 2: 10 consecutive calls dropped in the middle of the phrase. The second and later
    // ones wait for a session lock that the previous dropped call's decode request still holds.
    let middle_block = run(&cfg, pcm.as_ref(), |idx, total| {
        (total / 2..total / 2 + 10).contains(&idx)
    })
    .await;
    assert_eq!(middle_block, reference, "10 consecutive calls dropped");
    // Scenario 3: drop, drop, normal over the whole phrase.
    let drop_drop_normal = run(&cfg, pcm.as_ref(), |idx, _| idx % 3 != 2).await;
    assert_eq!(drop_drop_normal, reference, "drop, drop, normal");
}
