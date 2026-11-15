//! `finalize_utterance` must not drop the last word of an utterance.
//!
//! Symptom: the Sherpa `start:roundtrip-slow-final` smoke failed in CI with P2 "four five six"
//! finalized as "four five". The STT audio tap of that turn (`stt-in-*-utt2.wav`) holds the whole
//! phrase followed by about 1.3 s of near-silent inbound audio, and replaying it offline gave
//! "four five" every time. The same audio with 100 ms of zeros appended gave "four five six".
//! The streaming zipformer only decodes whole chunks: the frames after the last full chunk, and
//! the model's look-ahead for the last word, were never decoded because `finalize_utterance`
//! called `InputFinished` without any tail padding. Whether a last word survived depended on how
//! the utterance length fell on the chunk grid, so it was nondeterministic across runs.
//! Fixed by feeding zero tail padding before `InputFinished` (`FINALIZE_TAIL_PADDING_MS`).
//!
//! Fixture: `fixtures/four-five-six-last-word-16k-mono.pcm`, 2.68 s, 16 kHz mono s16le: the exact
//! STT input of the failing CI turn (Piper amy-low "four five six" received over WebRTC).
//!
//! Needs `SHERPA_STT_MODEL_PATH` (Kroko English zipformer); skips when it is unset or missing:
//!
//! ```bash
//! SHERPA_STT_MODEL_PATH=examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 \
//! cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test stt_finalize_last_word_test -- --nocapture
//! ```

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

const FIXTURE: &[u8] = include_bytes!("fixtures/four-five-six-last-word-16k-mono.pcm");
/// 20 ms of 16 kHz mono s16le, the agent's frame size.
const CHUNK_BYTES: usize = 640;
/// The fixture starts with about 300 ms of silence. Trimming 0..ALIGNMENTS x 20 ms of it moves
/// the utterance end across the recognizer's chunk grid; before the fix offset 0 gave "four five"
/// and offset 15 (300 ms) gave "four five si". Every alignment must keep all three words.
const ALIGNMENTS: usize = 16;
/// Each utterance runs on a fresh stream, so every repetition must give the same final.
const REPEATS: usize = 4;

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

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_keeps_the_last_word_of_the_captured_utterance() {
    let Ok(model_path) = std::env::var("SHERPA_STT_MODEL_PATH") else {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not set");
        return;
    };
    if !std::path::Path::new(&model_path).is_dir() {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not a directory: {model_path}");
        return;
    }
    assert_eq!(FIXTURE.len(), 85_760, "unexpected fixture size");

    let mut stt = SherpaFactory
        .create_stt(&stt_config(model_path))
        .expect("create_stt");
    stt.start().await.expect("stt start");

    let mut finals = Vec::new();
    for offset in (0..ALIGNMENTS).flat_map(|o| std::iter::repeat_n(o, REPEATS)) {
        let mut seen = Vec::new();
        for chunk in FIXTURE[offset * CHUNK_BYTES..].chunks(CHUNK_BYTES) {
            stt.push_and_poll(Bytes::copy_from_slice(chunk), &mut seen)
                .await
                .expect("push_and_poll");
        }
        stt.finalize_utterance().await.expect("finalize_utterance");
        while let Some(t) = stt.poll_transcript().await.expect("poll") {
            seen.push(t);
        }
        let text = seen
            .into_iter()
            .filter_map(|t| match t {
                SttTranscript::Final(text) => Some(text),
                SttTranscript::Partial(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        finals.push((offset, text));
    }
    stt.stop().await.expect("stt stop");

    for (offset, f) in &finals {
        eprintln!("lead trimmed {} ms: {f:?}", offset * 20);
    }
    let lost: Vec<_> = finals
        .iter()
        .filter(|(_, f)| words(f) != ["four", "five", "six"])
        .collect();
    assert!(
        lost.is_empty(),
        "finals lost words of the utterance (lead trimmed in 20 ms steps): {lost:?}"
    );
}

/// The tail padding is zeros: finalizing an utterance that carried only silence (or no audio at
/// all) must still give no words.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_of_silence_or_no_audio_gives_no_words() {
    let Ok(model_path) = std::env::var("SHERPA_STT_MODEL_PATH") else {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not set");
        return;
    };
    if !std::path::Path::new(&model_path).is_dir() {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not a directory: {model_path}");
        return;
    }

    let mut stt = SherpaFactory
        .create_stt(&stt_config(model_path))
        .expect("create_stt");
    stt.start().await.expect("stt start");

    // 1 s of zeros, then an utterance with no samples at all, then 1 s of zeros again.
    let mut texts = Vec::new();
    for silence_ms in [1000usize, 0, 1000] {
        let pcm = vec![0_u8; 32 * silence_ms];
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
        let text = seen
            .into_iter()
            .map(|t| match t {
                SttTranscript::Final(text) | SttTranscript::Partial(text) => text,
            })
            .collect::<Vec<_>>()
            .join(" ");
        texts.push((silence_ms, text));
    }
    stt.stop().await.expect("stt stop");

    for (silence_ms, text) in &texts {
        eprintln!("silence {silence_ms} ms: {text:?}");
    }
    assert!(
        texts.iter().all(|(_, text)| words(text).is_empty()),
        "silence-only utterances produced words: {texts:?}"
    );
}

/// End of speech in the fixture: the end of the last 20 ms frame whose peak is above
/// `SPEECH_PEAK` (the received tail after it is near-silent), in bytes.
const SPEECH_PEAK: i16 = 1000;
/// Trailing audio kept after the speech end, 40 ms steps up to 800 ms: every end position the
/// 640 ms tail padding could leave undecoded on the Kroko chunk grid (1.28 s steps).
const TAIL_STEPS: usize = 21;
const TAIL_STEP_BYTES: usize = 40 * 32;

fn speech_end_bytes() -> usize {
    let mut end = 0;
    for (index, frame) in FIXTURE.chunks(CHUNK_BYTES).enumerate() {
        let peak = frame
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b).saturating_abs())
            .max()
            .unwrap_or(0);
        if peak > SPEECH_PEAK {
            end = (index + 1) * CHUNK_BYTES;
        }
    }
    end
}

/// Final text of one utterance (`pcm` fed in 20 ms frames, then `finalize_utterance`) and the
/// `finalize_utterance` wall time in ms.
async fn finalize_text(
    stt: &mut Box<dyn node_webrtc_rust_speech::pipeline::SttProvider>,
    pcm: &[u8],
) -> (String, f64) {
    let mut seen = Vec::new();
    for chunk in pcm.chunks(CHUNK_BYTES) {
        stt.push_and_poll(Bytes::copy_from_slice(chunk), &mut seen)
            .await
            .expect("push_and_poll");
    }
    let started = std::time::Instant::now();
    stt.finalize_utterance().await.expect("finalize_utterance");
    let finalize_ms = started.elapsed().as_secs_f64() * 1000.0;
    while let Some(t) = stt.poll_transcript().await.expect("poll") {
        seen.push(t);
    }
    let text = seen
        .into_iter()
        .filter_map(|t| match t {
            SttTranscript::Final(text) => Some(text),
            SttTranscript::Partial(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    (text, finalize_ms)
}

/// `finalize_utterance` must decode speech that runs up to the very last sample. With only the
/// 640 ms tail padding, up to ~0.8 s of speech after the last full chunk of the Kroko model was
/// never decoded ("four five six" -> "four five"); the flush (`finalize_flush.rs`) decodes until
/// every sample was consumed. Covers the fixture cut at its speech end (no trailing silence) and
/// 40 ms steps of trailing audio up to 800 ms, for every lead trim of `ALIGNMENTS`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalize_decodes_speech_up_to_the_last_sample() {
    let Ok(model_path) = std::env::var("SHERPA_STT_MODEL_PATH") else {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not set");
        return;
    };
    if !std::path::Path::new(&model_path).is_dir() {
        eprintln!("skip: SHERPA_STT_MODEL_PATH is not a directory: {model_path}");
        return;
    }
    let speech_end = speech_end_bytes();
    assert!(
        speech_end > 32_000 && speech_end + TAIL_STEPS * TAIL_STEP_BYTES <= FIXTURE.len(),
        "speech end {speech_end} bytes leaves no room for the tail sweep"
    );

    let mut stt = SherpaFactory
        .create_stt(&stt_config(model_path))
        .expect("create_stt");
    stt.start().await.expect("stt start");

    let mut lost = Vec::new();
    let mut finalize_ms = Vec::new();
    for offset in 0..ALIGNMENTS {
        for tail in 0..TAIL_STEPS {
            let cut = speech_end + tail * TAIL_STEP_BYTES;
            let (text, ms) = finalize_text(&mut stt, &FIXTURE[offset * CHUNK_BYTES..cut]).await;
            finalize_ms.push(ms);
            if words(&text) != ["four", "five", "six"] {
                lost.push((offset * 20, tail * 40, text));
            }
        }
    }
    stt.stop().await.expect("stt stop");

    finalize_ms.sort_by(f64::total_cmp);
    eprintln!(
        "finalize_utterance ms: p50={:.1} max={:.1} (n={})",
        finalize_ms[finalize_ms.len() / 2],
        finalize_ms[finalize_ms.len() - 1],
        finalize_ms.len()
    );
    assert!(
        lost.is_empty(),
        "finals lost words (lead trimmed ms, trailing audio ms after speech end, final): {lost:?}"
    );
}
