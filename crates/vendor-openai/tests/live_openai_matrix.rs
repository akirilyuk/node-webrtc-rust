//! Live OpenAI STT/TTS matrix tests. Run with `OPENAI_LIVE=1` and `OPENAI_API_KEY` set.

#![cfg(feature = "live")]

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k, STT_MIN_BATCH_BYTES};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_openai::{DOCUMENTED_STT_MODELS, OpenAiFactory, OpenAiStt, OpenAiTts};
use tokio::sync::Mutex;

const COUNTING_PHRASE: &str =
    "one. two. three. four. five. six. seven. eight. nine. ten.";

/// Models that support file HTTP with `stream=true` (default VoiceAgent path), excluding Realtime-only.
const FILE_SSE_STT_MODELS: &[&str] = &[
    "gpt-4o-mini-transcribe",
    "gpt-4o-transcribe",
    "gpt-4o-transcribe-diarize",
    "gpt-transcribe",
];

const TTS_AUDIO_MODELS: &[&str] = &["tts-1", "tts-1-hd", "gpt-4o-mini-tts"];

static COUNTING_PCM: Mutex<Option<Bytes>> = Mutex::const_new(None);

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
    // Diarize does not support prompts (speech-to-text guide). Omit `language` so the
    // live connect matches a documented diarize request; VoiceAgent still forwards
    // `SttConfig.language` when the dashboard sets one.
    let language = if model == "gpt-4o-transcribe-diarize" {
        None
    } else {
        Some("en".into())
    };
    SttConfig {
        provider: SttVendor::Openai,
        model: Some(model.into()),
        model_path: None,
        language,
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        endpoint: None,
    }
}

fn tts_config(model: &str) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::Openai,
        model: Some(model.into()),
        model_path: None,
        voice: Some("alloy".into()),
        api_key: std::env::var("OPENAI_API_KEY").ok(),
        endpoint: None,
    }
}

async fn build_counting_pcm() -> Bytes {
    let factory = OpenAiFactory;
    let tts = factory
        .create_tts(&tts_config("gpt-4o-mini-tts"))
        .expect("tts for counting pcm");
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

async fn counting_pcm() -> Bytes {
    let mut guard = COUNTING_PCM.lock().await;
    if guard.is_none() {
        *guard = Some(build_counting_pcm().await);
    }
    guard.as_ref().expect("counting pcm").clone()
}

async fn drain_until_final(stt: &mut OpenAiStt) -> String {
    for _ in 0..48 {
        match stt.poll_transcript().await.expect("poll") {
            Some(SttTranscript::Final(text)) => return text,
            Some(SttTranscript::Partial(_)) => continue,
            None => break,
        }
    }
    panic!("expected SttTranscript::Final from vendor");
}

async fn run_finalize(stt: &mut OpenAiStt, pcm: Bytes, model: &str) -> String {
    stt.start()
        .await
        .map_err(|e| format!("model `{model}` start: {e}"))
        .expect("start");
    stt.push_audio(pcm)
        .await
        .map_err(|e| format!("model `{model}` push: {e}"))
        .expect("push");
    stt.finalize_utterance()
        .await
        .map_err(|e| format!("model `{model}` finalize: {e}"))
        .expect("finalize");
    drain_until_final(stt).await
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
async fn live_stt_default_transport_every_documented_model() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm().await;
    for model in DOCUMENTED_STT_MODELS {
        let mut stt = OpenAiStt::new(&stt_config(model))
            .map_err(|e| format!("model `{model}` OpenAiStt::new: {e}"))
            .expect("stt new");
        let text = run_finalize(&mut stt, pcm.clone(), model).await;
        assert_counting_words(&text);
        stt.stop().await.ok();
    }
}

#[tokio::test]
async fn live_stt_file_json_sse_capable_models() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm().await;
    for model in FILE_SSE_STT_MODELS {
        let mut stt = OpenAiStt::new_force_http_file_json(&stt_config(model))
            .map_err(|e| format!("model `{model}` new_force_http_file_json: {e}"))
            .expect("stt");
        let text = run_finalize(&mut stt, pcm.clone(), model).await;
        assert_counting_words(&text);
        stt.stop().await.ok();
    }
}

#[tokio::test]
async fn live_stt_realtime_committed_gpt_transcribe() {
    if skip_live() {
        return;
    }
    let pcm = counting_pcm().await;
    let model = "gpt-transcribe";
    let mut stt = OpenAiStt::new_force_realtime(&stt_config(model))
        .map_err(|e| format!("model `{model}` new_force_realtime: {e}"))
        .expect("stt");
    let text = run_finalize(&mut stt, pcm, model).await;
    assert_counting_words(&text);
    stt.stop().await.ok();
}

#[tokio::test]
async fn live_tts_audio_and_progressive() {
    if skip_live() {
        return;
    }
    let factory = OpenAiFactory;
    for model in TTS_AUDIO_MODELS {
        let tts = factory
            .create_tts(&tts_config(model))
            .map_err(|e| format!("model `{model}` create_tts: {e}"))
            .expect("tts");
        let chunks = tts
            .synthesize("hello")
            .await
            .map_err(|e| format!("model `{model}` synthesize: {e}"))
            .expect("full body");
        assert!(!chunks.is_empty(), "model `{model}`");
        assert!(!chunks[0].pcm.is_empty(), "model `{model}`");
        let prog = tts
            .synthesize_progressive("hello", None)
            .await
            .map_err(|e| format!("model `{model}` synthesize_progressive: {e}"))
            .expect("progressive");
        assert!(!prog.is_empty(), "model `{model}` progressive");
    }

    // Same delivery plan as gpt-4o-mini-tts; run when the dated snapshot is available.
    let optional = "gpt-4o-mini-tts-2025-12-15";
    if let Ok(tts) = factory.create_tts(&tts_config(optional)) {
        if let Ok(chunks) = tts.synthesize("hello").await {
            if !chunks.is_empty() && !chunks[0].pcm.is_empty() {
                let _ = tts.synthesize_progressive("hello", None).await;
            }
        }
    }
}

#[tokio::test]
async fn live_tts_sse_allow_and_deny() {
    if skip_live() {
        return;
    }
    let mini = OpenAiTts::new(&tts_config("gpt-4o-mini-tts")).expect("tts");
    let chunks = mini
        .synthesize_pcm_sse("hello")
        .await
        .expect("gpt-4o-mini-tts sse pcm");
    assert!(!chunks.is_empty());
    assert!(chunks.iter().any(|c| !c.pcm.is_empty()));

    for model in ["tts-1", "tts-1-hd"] {
        let tts = OpenAiTts::new(&tts_config(model)).expect("tts");
        let err = tts
            .synthesize_pcm_sse("hello")
            .await
            .expect_err(&format!("model `{model}` must reject SSE"));
        let msg = err.to_string();
        assert!(
            !msg.is_empty(),
            "model `{model}` SSE error should be descriptive"
        );
    }
}
