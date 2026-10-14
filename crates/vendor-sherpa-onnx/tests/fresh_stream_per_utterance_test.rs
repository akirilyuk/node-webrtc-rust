//! Every utterance must start on a fresh online stream.
//!
//! Symptom: staging load-test clients read `"okay okay seven three two six ten"` for a reply that
//! says "okay" once (1.8-2.2 % of turns). The audio reached STT exactly once; the recognizer
//! stream was reused across utterances. A finalized stream got `InputFinished`, and sherpa's
//! `Reset` keeps the encoder state and the last tokens as context, so the next utterance on the
//! same stream could repeat its first word. Fixed by creating a new `OnlineStream` in
//! `finalize_utterance`.
//!
//! Fixture: `fixtures/okay-doubled-first-word-16k-mono.pcm`, 2.84 s of Piper amy-medium (speed 0.8)
//! "Okay. two, eight five seven ten", 16 kHz mono s16le (the agent's `stereo_48k_to_mono_16k`
//! of a PCM that doubled 7 of 20 times when replayed on one reused agent).
//!
//! One `SherpaStt`, 20 utterances: lead-in silence, the fixture in 20 ms chunks, tail silence,
//! `finalize_utterance`, read the final. No final may start with two "okay"s and all 20 finals
//! must be identical after normalisation.
//!
//! Needs `SHERPA_STT_MODEL_PATH` (Kroko English zipformer); skips when it is unset or missing:
//!
//! ```bash
//! SHERPA_STT_MODEL_PATH=examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 \
//! cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test fresh_stream_per_utterance_test -- --nocapture
//! ```

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pcm::silence_mono_s16le_bytes;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

const FIXTURE: &[u8] = include_bytes!("fixtures/okay-doubled-first-word-16k-mono.pcm");
const UTTERANCES: usize = 20;
/// 20 ms of 16 kHz mono s16le.
const CHUNK_BYTES: usize = 640;
/// What the agent has in front of a gated utterance: the 700 ms pre-roll ring.
const LEAD_IN_MS: u32 = 700;
/// What follows the speech before the final: gate hold 1000 ms + endpoint tail 600 ms.
const TAIL_MS: u32 = 1600;

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

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

fn doubled_first_word(text: &str) -> bool {
    let w = words(text);
    w.len() >= 2 && w[0] == "okay" && w[1] == "okay"
}

fn normalized(text: &str) -> String {
    words(text).join(" ")
}

async fn push_all(stt: &mut Box<dyn SttProvider>, pcm: &[u8], seen: &mut Vec<SttTranscript>) {
    for chunk in pcm.chunks(CHUNK_BYTES) {
        stt.push_and_poll(Bytes::copy_from_slice(chunk), seen)
            .await
            .expect("push_and_poll");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_utterance_on_one_sherpa_stt_starts_on_a_fresh_stream() {
    let Ok(model_path) = std::env::var("SHERPA_STT_MODEL_PATH") else {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not set");
        return;
    };
    if !std::path::Path::new(&model_path).is_dir() {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not a directory: {model_path}");
        return;
    }
    assert!(
        (80_000..200_000).contains(&FIXTURE.len()),
        "unexpected fixture size {}",
        FIXTURE.len()
    );

    let mut stt = SherpaFactory
        .create_stt(&stt_config(model_path))
        .expect("create_stt");
    stt.start().await.expect("stt start");

    let lead_in = silence_mono_s16le_bytes(LEAD_IN_MS);
    let tail = silence_mono_s16le_bytes(TAIL_MS);
    let mut finals: Vec<String> = Vec::new();
    for _ in 0..UTTERANCES {
        let mut seen = Vec::new();
        push_all(&mut stt, lead_in.as_ref(), &mut seen).await;
        push_all(&mut stt, FIXTURE, &mut seen).await;
        push_all(&mut stt, tail.as_ref(), &mut seen).await;
        stt.finalize_utterance().await.expect("finalize_utterance");
        while let Some(t) = stt.poll_transcript().await.expect("poll") {
            seen.push(t);
        }
        // Endpointing may close the utterance before `finalize_utterance`; join every final.
        let text = seen
            .into_iter()
            .filter_map(|t| match t {
                SttTranscript::Final(text) => Some(text),
                SttTranscript::Partial(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        finals.push(text);
    }
    stt.stop().await.expect("stt stop");

    let doubled = finals.iter().filter(|f| doubled_first_word(f)).count();
    eprintln!("doubled={doubled}/{UTTERANCES}");
    for (i, f) in finals.iter().enumerate() {
        eprintln!("final #{i}: {f:?}");
    }
    assert_eq!(
        doubled, 0,
        "first word doubled in {doubled}/{UTTERANCES} finals: {finals:?}"
    );
    let first = normalized(&finals[0]);
    assert!(!first.is_empty(), "first utterance produced no final");
    for (i, f) in finals.iter().enumerate() {
        assert_eq!(
            normalized(f),
            first,
            "final #{i} differs from #0 (stream state leaked between utterances): {finals:?}"
        );
    }
}
