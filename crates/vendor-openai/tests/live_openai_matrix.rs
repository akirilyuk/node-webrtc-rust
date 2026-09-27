//! Live OpenAI STT/TTS matrix tests. Run with `OPENAI_LIVE=1` and `OPENAI_API_KEY` set.

#![cfg(feature = "live")]

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k, STT_MIN_BATCH_BYTES};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_openai::{OpenAiFactory, OpenAiStt};

const COUNTING_PHRASE: &str =
    "one. two. three. four. five. six. seven. eight. nine. ten.";

fn live_enabled() -> bool {
    std::env::var("OPENAI_LIVE").as_deref() == Ok("1")
        && std::env::var("OPENAI_API_KEY")
            .map(|k| !k.is_empty())
            .unwrap_or(false)
}

fn skip_live() -> bool {
    !live_enabled()
}

fn stt_config(model: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::Openai,
        model: Some(model.into()),
        model_path: None,
        language: Some("en".into()),
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        endpoint: None,
    }
}

fn pcm_above_min() -> Bytes {
    Bytes::from(vec![0_u8; STT_MIN_BATCH_BYTES + 4096])
}

async fn counting_pcm_via_tts() -> Bytes {
    let factory = OpenAiFactory;
    let tts = factory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Openai,
            model: Some("gpt-4o-mini-tts".into()),
            model_path: None,
            voice: Some("alloy".into()),
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            endpoint: None,
        })
        .expect("tts");
    let chunks = tts.synthesize(COUNTING_PHRASE).await.expect("tts synth");
    let mut pcm = Vec::new();
    for chunk in chunks {
        pcm.extend_from_slice(&chunk.pcm);
    }
    let mono = stereo_48k_to_mono_16k(&pcm);
    let mut mono16k = i16_samples_to_bytes(&mono).to_vec();
    if mono16k.len() < STT_MIN_BATCH_BYTES {
        mono16k.resize(STT_MIN_BATCH_BYTES + 64, 0);
    }
    Bytes::from(mono16k)
}

async fn run_http_finalize(
    model: &str,
    pcm: Bytes,
    force_realtime: bool,
) -> Option<SttTranscript> {
    let mut stt = if force_realtime {
        OpenAiStt::new_force_realtime(&stt_config(model)).expect("stt")
    } else {
        OpenAiStt::new(&stt_config(model)).expect("stt")
    };
    stt.start().await.expect("start");
    stt.push_audio(pcm).await.expect("push");
    stt.finalize_utterance().await.expect("finalize");
    let mut last = None;
    for _ in 0..8 {
        if let Some(t) = stt.poll_transcript().await.expect("poll") {
            let is_final = matches!(&t, SttTranscript::Final(_));
            last = Some(t);
            if is_final {
                break;
            }
        }
    }
    stt.stop().await.ok();
    last
}

fn assert_counting_words(text: &str) {
    let lower = text.to_lowercase();
    let word_hits = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"]
        .iter()
        .filter(|w| lower.contains(*w))
        .count();
    let digit_hits = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]
        .iter()
        .filter(|d| lower.contains(*d))
        .count();
    assert!(
        word_hits >= 4 || digit_hits >= 4,
        "expected counting words or digits in `{text}` (words={word_hits} digits={digit_hits})"
    );
}

#[tokio::test]
async fn live_file_json_whisper1() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm_via_tts().await;
    let t = run_http_finalize("whisper-1", pcm, false).await;
    let SttTranscript::Final(text) = t.expect("final") else {
        panic!("expected final");
    };
    assert_counting_words(&text);
}

#[tokio::test]
async fn live_file_sse_mini_and_gpt_transcribe() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm_via_tts().await;
    for model in ["gpt-4o-mini-transcribe", "gpt-transcribe"] {
        let mut stt = OpenAiStt::new(&stt_config(model)).expect("stt");
        stt.start().await.expect("start");
        stt.push_audio(pcm.clone()).await.expect("push");
        stt.finalize_utterance().await.expect("finalize");
        let mut saw_delta_or_done = false;
        let mut final_text = None;
        for _ in 0..32 {
            match stt.poll_transcript().await.expect("poll") {
                Some(SttTranscript::Partial(_)) => saw_delta_or_done = true,
                Some(SttTranscript::Final(text)) => {
                    saw_delta_or_done = true;
                    final_text = Some(text);
                    break;
                }
                None => break,
            }
        }
        assert!(saw_delta_or_done, "expected SSE partial or final for {model}");
        if let Some(text) = final_text {
            assert_counting_words(&text);
        }
        stt.stop().await.ok();
    }
}

#[tokio::test]
async fn live_realtime_gpt_live_transcribe() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm_via_tts().await;
    let t = run_http_finalize("gpt-live-transcribe", pcm, false).await;
    if let Some(SttTranscript::Final(text)) = t {
        assert_counting_words(&text);
    }
}

#[tokio::test]
async fn live_realtime_committed_gpt_transcribe() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm_via_tts().await;
    let t = run_http_finalize("gpt-transcribe", pcm, true).await;
    if let Some(SttTranscript::Final(text)) = t {
        assert_counting_words(&text);
    }
}

#[tokio::test]
async fn live_tts_audio_and_full_body() {
    if skip_live() {
        return;
    }
    let factory = OpenAiFactory;
    for model in ["tts-1", "gpt-4o-mini-tts"] {
        let tts = factory
            .create_tts(&TtsConfig {
                provider: TtsVendor::Openai,
                model: Some(model.into()),
                model_path: None,
                voice: Some("alloy".into()),
                api_key: std::env::var("OPENAI_API_KEY").ok(),
                endpoint: None,
            })
            .expect("tts");
        let chunks = tts.synthesize("hello").await.expect("full body");
        assert!(!chunks.is_empty());
        assert!(!chunks[0].pcm.is_empty());
        let prog = tts
            .synthesize_progressive("hello", None)
            .await
            .expect("progressive");
        assert!(!prog.is_empty());
    }
}
