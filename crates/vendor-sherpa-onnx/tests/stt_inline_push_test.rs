//! Inline STT push: same finals as the hop-per-message path, far fewer blocking hops.
//!
//! `#[ignore]` for default `cargo test` (needs STT + TTS weights). Run with `--test-threads=1`:
//! the hop counter is process-wide (own test binary, one test).

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{
    i16_samples_to_bytes, silence_mono_s16le_bytes, stereo_48k_to_mono_16k,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::{
    reset_sherpa_push_blocking_hops, sherpa_push_blocking_hops, SherpaFactory, SherpaStt,
};

const PHRASES: [&str; 4] = [
    "alpha one two three",
    "bravo four five six",
    "delta seven eight nine",
    "echo ten eleven twelve",
];
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

/// Pushes `pcm` in 20 ms `push_and_poll` messages, then finalizes and drains. Returns the final texts in order, the number of messages, and the push blocking hops counted during the pushes.
async fn run(cfg: &SttConfig, pcm: &[u8], inline: bool) -> (Vec<String>, u64, u64) {
    let mut stt = SherpaStt::new(cfg);
    stt.set_inline_push(inline);
    stt.start().await.expect("stt start");
    reset_sherpa_push_blocking_hops();
    let mut seen = Vec::new();
    let mut messages = 0u64;
    for chunk in pcm.chunks(CHUNK_BYTES) {
        messages += 1;
        stt.push_and_poll(Bytes::copy_from_slice(chunk), &mut seen)
            .await
            .expect("push_and_poll");
    }
    let hops = sherpa_push_blocking_hops();
    stt.finalize_utterance().await.expect("finalize_utterance");
    while let Some(t) = stt.poll_transcript().await.expect("poll") {
        seen.push(t);
    }
    stt.stop().await.expect("stt stop");
    let finals = seen
        .into_iter()
        .filter_map(|t| match t {
            SttTranscript::Final(text) => Some(text),
            SttTranscript::Partial(_) => None,
        })
        .collect();
    (finals, messages, hops)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires SHERPA_STT_MODEL_PATH and SHERPA_TTS_MODEL_PATH"]
async fn inline_push_matches_hop_per_message_with_fewer_hops() {
    let stt_path = std::env::var("SHERPA_STT_MODEL_PATH").expect("set SHERPA_STT_MODEL_PATH");
    let cfg = stt_config(stt_path);
    let pcm = build_pcm().await;

    let (off_finals, messages, off_hops) = run(&cfg, &pcm, false).await;
    let (on_finals, on_messages, on_hops) = run(&cfg, &pcm, true).await;
    eprintln!("inline push: messages={messages} hops_off={off_hops} hops_on={on_hops}");

    assert_eq!(messages, on_messages);
    assert!(!off_finals.is_empty(), "no finals in the reference run");
    assert_eq!(on_finals, off_finals, "finals differ between modes");
    assert_eq!(off_hops, messages, "switch off: one hop per message");
    assert!(
        on_hops * 2 <= messages,
        "switch on: hops {on_hops} must be at most half of {messages} messages"
    );
}
