//! Latency bench against the real API (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key or a transcript.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5). Without `OPENAI_API_KEY` the bench skips, unless
//! `OPENAI_LIVE_REQUIRED=1`.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_live_support::audio::STT_PHRASE;
use node_webrtc_rust_vendor_live_support::bench::{
    bench_stt, bench_stt_warm, bench_tts, runs_from_env,
};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, STT_CLIP_TAIL_MS};
use node_webrtc_rust_vendor_openai::{documented_realtime_model, OpenAiFactory, SttRealtimeMode};
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "openai";

fn tts(key: &str, transport: &str) -> Box<dyn TtsProvider> {
    OpenAiFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Openai,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            voice: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::parse(transport).expect("transport")),
        })
        .unwrap_or_else(|err| panic!("tts config: {err}"))
}

fn stt(key: &str, model: &str) -> Box<dyn SttProvider> {
    OpenAiFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Openai,
            model: Some(model.to_string()),
            model_path: None,
            language: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            transport: None,
        })
        .unwrap_or_else(|err| panic!("stt config: {err}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench() {
    let Some(mut env) = live_env(ID, &["OPENAI_API_KEY"]) else {
        return;
    };
    let key = env.remove("OPENAI_API_KEY").expect("key");
    let runs = runs_from_env(5);
    let tts_model = voice_catalog::default_tts_model(ID).expect("default tts model");
    let stt_model = voice_catalog::default_stt_model(ID).expect("default stt model");
    let realtime_model = documented_realtime_model(SttRealtimeMode::Live);

    for transport in voice_catalog::tts_transports(ID) {
        bench_tts(ID, transport, tts_model, || tts(&key, transport), runs)
            .await
            .unwrap_or_else(|err| panic!("{ID} {transport} tts bench failed: {err}"));
    }

    // The STT phrase is synthesized once with the vendor's own default TTS.
    let speech: Vec<u8> = tts(&key, "auto")
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stereo = [
        silence_stereo_48k(1000),
        speech,
        silence_stereo_48k(STT_CLIP_TAIL_MS),
    ]
    .concat();

    // `whisper-1` is one file upload per utterance, so a second utterance on the same provider
    // is a normal request (the pooled connection is reused): cold and warm.
    bench_stt_warm(
        ID,
        "http",
        stt_model,
        || stt(&key, stt_model),
        &stereo,
        runs,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} {stt_model} stt bench failed: {err}"));

    // The realtime model opens a session per utterance: cold only.
    bench_stt(
        ID,
        "websocket",
        realtime_model,
        || stt(&key, realtime_model),
        &stereo,
        runs,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} {realtime_model} stt bench failed: {err}"));
}
