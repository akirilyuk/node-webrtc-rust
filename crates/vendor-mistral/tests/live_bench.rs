//! Latency bench against the real API (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key or a transcript.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5). Without `MISTRAL_API_KEY` the bench skips, unless
//! `MISTRAL_LIVE_REQUIRED=1`. `MISTRAL_BASE_URL` picks the endpoint (global, EU or US); the
//! chosen URL is printed once as `mistral_bench_endpoint {"url":..}`.
//!
//! Legs:
//! - TTS: per model (`voxtral-mini-tts-realtime-3-0`, `voxtral-mini-tts-3-0`), transport `http`,
//!   texts `short` and `sentence`, cold and warm.
//! - STT: per `target_streaming_delay_ms` in 160, 240, 480. The record's `model` is
//!   `<model>@delay<ms>` so summary.md shows the delay. `metrics.matched_words` is the word
//!   accuracy against the 9 `STT_WORDS`.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider};
use node_webrtc_rust_vendor_live_support::audio::STT_PHRASE;
use node_webrtc_rust_vendor_live_support::bench::{bench_stt, bench_tts, runs_from_env};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, STT_CLIP_TAIL_MS};
use node_webrtc_rust_vendor_mistral::__test::stt_with_streaming_delay;
use node_webrtc_rust_vendor_mistral::{
    resolve_base_url_from_env, MistralTts, DEFAULT_STT_MODEL, DEFAULT_TTS_MODEL,
};
use serde_json::json;

const TTS_MODELS: [&str; 2] = ["voxtral-mini-tts-realtime-3-0", "voxtral-mini-tts-3-0"];
const STT_DELAYS_MS: [u32; 3] = [160, 240, 480];

fn tts(key: &str, model: &str) -> Box<dyn TtsProvider> {
    Box::new(
        MistralTts::new(&TtsConfig {
            provider: TtsVendor::Mistral,
            model: Some(model.to_string()),
            model_path: None,
            voice: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        })
        .unwrap_or_else(|err| panic!("tts config: {err}")),
    )
}

fn stt(key: &str, delay_ms: u32) -> Box<dyn SttProvider> {
    Box::new(
        stt_with_streaming_delay(
            &SttConfig {
                provider: SttVendor::Mistral,
                model: None,
                model_path: None,
                language: Some("en".into()),
                api_key: Some(key.to_string()),
                endpoint: None,
                transport: None,
            },
            delay_ms,
        )
        .unwrap_or_else(|err| panic!("stt config: {err}")),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench() {
    let Some(mut env) = live_env("mistral", &["MISTRAL_API_KEY"]) else {
        return;
    };
    let key = env.remove("MISTRAL_API_KEY").expect("key");
    let runs = runs_from_env(5);
    println!(
        "mistral_bench_endpoint {}",
        json!({"url": resolve_base_url_from_env(None)})
    );

    for model in TTS_MODELS {
        bench_tts("mistral", "http", model, || tts(&key, model), runs)
            .await
            .unwrap_or_else(|err| panic!("mistral {model} tts bench failed: {err}"));
    }

    // The STT phrase is synthesized once with the default TTS model.
    let speech: Vec<u8> = tts(&key, &DEFAULT_TTS_MODEL)
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("mistral phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stereo = [
        silence_stereo_48k(1000),
        speech,
        silence_stereo_48k(STT_CLIP_TAIL_MS),
    ]
    .concat();
    for delay_ms in STT_DELAYS_MS {
        let model = format!("{}@delay{delay_ms}", *DEFAULT_STT_MODEL);
        bench_stt(
            "mistral",
            "websocket",
            &model,
            || stt(&key, delay_ms),
            &stereo,
            runs,
        )
        .await
        .unwrap_or_else(|err| panic!("mistral stt bench (delay {delay_ms}) failed: {err}"));
    }
}
