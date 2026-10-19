//! Intelligibility of tiny sentence fragments under the production TTS path.
//!
//! `split_sentences` merges tiny pieces (`I. am. happy.`) because espeak-ng reads the lowercase
//! form as ONE sentence, while a lone `I.` is spoken as a letter. Sentence-start capitalization
//! must not undo that: with `I. Am. Happy.` espeak makes three sentences again and the phrase is
//! no longer recognised ("Ing have been", "I am happied").
//!
//! This test synthesizes a phrase with the default `sentenceCase` (auto) path, transcribes the
//! audio with the local Sherpa streaming STT (offline: feed all PCM, then finalize), N times, and
//! counts transcripts whose word similarity to the expected text is below 0.9.
//!
//! Kept `#[ignore]` (needs STT + TTS weights). Run:
//!
//! ```text
//! SHERPA_STT_MODEL_PATH=examples/voice-agent-local-sherpa/.models/sherpa-onnx-streaming-zipformer-en-kroko-2025-08-06 \
//! SHERPA_TTS_MODEL_PATH=examples/voice-agent-local-sherpa/.models/vits-piper-en_US-amy-low \
//! TTS_FRAGMENT_RUNS=20 \
//!   cargo test -p node-webrtc-rust-vendor-sherpa-onnx --test tts_fragment_intelligibility_test \
//!   -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Optional env: `TTS_FRAGMENT_PHRASE` (default `I. am. happy.`), `TTS_FRAGMENT_EXPECTED`
//! (default `i am happy`), `TTS_FRAGMENT_SENTENCE_CASE` (`auto` default | `off`),
//! `TTS_FRAGMENT_MAX_FAILURES` (when set, the test asserts at most that many failures).

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SentenceCase, SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{
    i16_samples_to_bytes, silence_mono_s16le_bytes, stereo_48k_to_mono_16k,
};
use node_webrtc_rust_speech::pipeline::{SttTranscript, VendorFactory};
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

/// 60 ms of 16 kHz mono s16le.
const CHUNK_BYTES: usize = 1920;
const MIN_SIMILARITY: f64 = 0.9;

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// `1 - word_edit_distance / max(len)`; 1.0 means the same words.
fn word_similarity(expected: &[String], heard: &[String]) -> f64 {
    let longest = expected.len().max(heard.len());
    if longest == 0 {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=heard.len()).collect();
    for (i, e) in expected.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, h) in heard.iter().enumerate() {
            let cost = usize::from(e != h);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    1.0 - prev[heard.len()] as f64 / longest as f64
}

#[test]
fn word_similarity_scores_known_cases() {
    let want = normalize("i am happy");
    assert_eq!(word_similarity(&want, &normalize("I am happy.")), 1.0);
    assert!(word_similarity(&want, &normalize("I am happied")) < MIN_SIMILARITY);
    assert!(word_similarity(&want, &normalize("Ing have been")) < MIN_SIMILARITY);
}

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

async fn transcribe(cfg: &SttConfig, pcm: &[u8]) -> String {
    let mut stt = SherpaFactory.create_stt(cfg).expect("create_stt");
    stt.start().await.expect("stt start");
    let mut seen: Vec<SttTranscript> = Vec::new();
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
    let finals: Vec<String> = seen
        .iter()
        .filter_map(|t| match t {
            SttTranscript::Final(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            _ => None,
        })
        .collect();
    if !finals.is_empty() {
        return finals.join(" ");
    }
    seen.iter()
        .rev()
        .find_map(|t| match t {
            SttTranscript::Partial(text) if !text.trim().is_empty() => {
                Some(text.trim().to_string())
            }
            _ => None,
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires SHERPA_STT_MODEL_PATH (streaming zipformer) and SHERPA_TTS_MODEL_PATH (Piper)"]
async fn tiny_fragments_are_recognised_after_synthesis() {
    let (Ok(stt_path), Ok(tts_path)) = (
        std::env::var("SHERPA_STT_MODEL_PATH"),
        std::env::var("SHERPA_TTS_MODEL_PATH"),
    ) else {
        eprintln!("SHERPA_STT_MODEL_PATH / SHERPA_TTS_MODEL_PATH not set; skipping");
        return;
    };
    let runs: usize = std::env::var("TTS_FRAGMENT_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let phrase = std::env::var("TTS_FRAGMENT_PHRASE").unwrap_or_else(|_| "I. am. happy.".into());
    let expected_text =
        std::env::var("TTS_FRAGMENT_EXPECTED").unwrap_or_else(|_| "i am happy".into());
    let expected = normalize(&expected_text);
    let sentence_case = match std::env::var("TTS_FRAGMENT_SENTENCE_CASE").as_deref() {
        Ok("off") => SentenceCase::Off,
        _ => SentenceCase::Auto,
    };

    let tts = SherpaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(tts_path),
            voice: Some("0".into()),
            api_key: None,
            endpoint: None,
            sentence_case,
            transport: None,
        })
        .expect("create TTS");
    let stt_cfg = stt_config(stt_path);
    let silence = silence_mono_s16le_bytes(1500);
    let lead_in = silence_mono_s16le_bytes(300);

    // Warm-up: loads the engine, not counted.
    let _ = tts.synthesize("Warm up.").await;

    let mut failures = 0usize;
    for run in 0..runs {
        let chunks = tts.synthesize(&phrase).await.expect("piper synthesis");
        let mut stereo = Vec::new();
        for chunk in &chunks {
            stereo.extend_from_slice(chunk.pcm.as_ref());
        }
        let mono = i16_samples_to_bytes(&stereo_48k_to_mono_16k(&stereo));
        let mut pcm: Vec<u8> = Vec::new();
        pcm.extend_from_slice(lead_in.as_ref());
        pcm.extend_from_slice(mono.as_ref());
        pcm.extend_from_slice(silence.as_ref());

        let heard = transcribe(&stt_cfg, &pcm).await;
        let similarity = word_similarity(&expected, &normalize(&heard));
        let failed = similarity < MIN_SIMILARITY;
        if failed {
            failures += 1;
        }
        println!(
            "run {run:02}: {} similarity={similarity:.2} heard={heard:?}",
            if failed { "FAIL" } else { "ok  " }
        );
    }
    println!(
        "SUMMARY phrase={phrase:?} sentence_case={} runs={runs} failures={failures}",
        sentence_case.as_wire()
    );
    if let Some(max) = std::env::var("TTS_FRAGMENT_MAX_FAILURES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        assert!(
            failures <= max,
            "{failures} of {runs} runs were not recognised as {expected_text:?} (max {max})"
        );
    }
}
