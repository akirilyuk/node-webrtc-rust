//! `push_and_poll` must yield the same transcripts as `push_audio` + `poll_transcript` loops,
//! while reading the recognizer result fewer times.
//!
//! `#[ignore]` for default `cargo test` (needs STT + TTS weights). CI runs it via
//! `scripts/ci/run-sherpa-example-ci.sh` (`run_rust_ignored`). Run with `--test-threads=1`: the
//! `get_result` counter is process-wide.

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{
    i16_samples_to_bytes, silence_mono_s16le_bytes, stereo_48k_to_mono_16k,
};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{
    reset_sherpa_get_result_count, sherpa_get_result_count, SherpaFactory,
};

const PHRASES: [&str; 4] = [
    "alpha one two three",
    "bravo four five six",
    "delta seven eight nine",
    "echo ten eleven twelve",
];
/// 60 ms of 16 kHz mono s16le.
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

/// Four phrases, each followed by 1.5 s of silence so the endpoint rules fire between them.
async fn build_pcm() -> Vec<u8> {
    let silence = silence_mono_s16le_bytes(1500);
    let mut pcm: Vec<u8> = Vec::new();
    for phrase in PHRASES {
        let chunk = synthesize_speech_pcm_16k(phrase).await;
        assert!(
            chunk.len() >= 3200,
            "phrase {phrase:?} produced too little PCM"
        );
        pcm.extend_from_slice(chunk.as_ref());
        pcm.extend_from_slice(silence.as_ref());
    }
    pcm
}

/// Returns the transcripts and the number of `poll_transcript` calls made. Before the skip in
/// `poll_transcript` every poll call that did not return a queued final read the result, so the
/// call count is the read count of the old behavior.
async fn transcribe_push_then_poll(cfg: &SttConfig, pcm: &[u8]) -> (Vec<SttTranscript>, u64) {
    let mut stt = SherpaFactory.create_stt(cfg).expect("create_stt");
    stt.start().await.expect("stt start");
    let mut seen = Vec::new();
    let mut poll_calls = 0u64;
    for chunk in pcm.chunks(CHUNK_BYTES) {
        stt.push_audio(Bytes::copy_from_slice(chunk))
            .await
            .expect("push_audio");
        loop {
            poll_calls += 1;
            match stt.poll_transcript().await.expect("poll") {
                Some(t) => seen.push(t),
                None => break,
            }
        }
    }
    stt.finalize_utterance().await.expect("finalize_utterance");
    loop {
        poll_calls += 1;
        match stt.poll_transcript().await.expect("poll") {
            Some(t) => seen.push(t),
            None => break,
        }
    }
    stt.stop().await.expect("stt stop");
    (seen, poll_calls)
}

async fn transcribe_push_and_poll(cfg: &SttConfig, pcm: &[u8]) -> Vec<SttTranscript> {
    let mut stt = SherpaFactory.create_stt(cfg).expect("create_stt");
    stt.start().await.expect("stt start");
    let mut seen = Vec::new();
    for chunk in pcm.chunks(CHUNK_BYTES) {
        stt.push_and_poll(Bytes::copy_from_slice(chunk), &mut seen)
            .await
            .expect("push_and_poll");
    }
    stt.finalize_utterance().await.expect("finalize_utterance");
    while let Some(t) = stt.poll_transcript().await.expect("poll") {
        seen.push(t);
    }
    stt.stop().await.expect("stt stop");
    seen
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
async fn push_and_poll_matches_push_then_poll() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let cfg = stt_config(stt_path);
    let pcm = build_pcm().await;

    reset_sherpa_get_result_count();
    let (old, old_poll_calls) = transcribe_push_then_poll(&cfg, &pcm).await;
    let skip_reads = sherpa_get_result_count();

    reset_sherpa_get_result_count();
    let new = transcribe_push_and_poll(&cfg, &pcm).await;
    let new_reads = sherpa_get_result_count();

    // The old behavior read once per poll call (minus the one finalize read, counted below).
    let old_reads = old_poll_calls;
    eprintln!(
        "get_result reads: old(one per poll call)={old_reads} push+poll with skip={skip_reads} \
         push_and_poll={new_reads} transcripts={}",
        old.len()
    );
    assert!(
        old.iter().any(|t| matches!(t, SttTranscript::Partial(_))),
        "reference run produced no partials: {old:?}"
    );
    assert!(
        old.iter().any(|t| matches!(t, SttTranscript::Final(_))),
        "reference run produced no finals: {old:?}"
    );
    assert_eq!(new, old, "push_and_poll transcripts differ from push+poll");
    assert!(
        new_reads <= skip_reads,
        "push_and_poll must not read more than push+poll with the skip (skip={skip_reads}, new={new_reads})"
    );
    assert!(
        new_reads < old_reads,
        "push_and_poll must read the result fewer times (old={old_reads}, new={new_reads})"
    );
}
