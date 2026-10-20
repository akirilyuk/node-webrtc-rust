//! Latency bench against the real API (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key or a transcript.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5). Without `FISH_API_KEY` the bench skips, unless
//! `FISH_LIVE_REQUIRED=1` (vendor id `fish`).
//!
//! `NWR_BENCH_TRANSPORT` (`all` by default, or a comma list such as `http,websocket`) selects the
//! transports to measure.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_fishaudio::FishAudioFactory;
use node_webrtc_rust_vendor_live_support::audio::STT_PHRASE;
use node_webrtc_rust_vendor_live_support::bench::{
    bench_stt, bench_tts, runs_from_env, transports_from_env,
};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, STT_CLIP_TAIL_MS};
use node_webrtc_rust_voice_catalog as voice_catalog;

/// Catalog id (the live-env vendor id is `fish`).
const ID: &str = "fishaudio";

fn tts(key: &str, transport: &str) -> Box<dyn TtsProvider> {
    FishAudioFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Fishaudio,
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

fn stt(key: &str) -> Box<dyn SttProvider> {
    FishAudioFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Fishaudio,
            model: voice_catalog::default_stt_model(ID).map(str::to_string),
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
    let Some(mut env) = live_env("fish", &["FISH_API_KEY"]) else {
        return;
    };
    let key = env.remove("FISH_API_KEY").expect("key");
    let runs = runs_from_env(5);
    let tts_model = voice_catalog::default_tts_model(ID).expect("default tts model");
    let stt_model = voice_catalog::default_stt_model(ID).expect("default stt model");

    // `http` and `websocket`, each set explicitly (no HTTP-to-WS fallback is measured).
    for transport in transports_from_env(voice_catalog::tts_transports(ID)) {
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
    if transports_from_env(&["websocket"]).contains(&"websocket") {
        bench_stt(ID, "websocket", stt_model, || stt(&key), &stereo, runs)
            .await
            .unwrap_or_else(|err| panic!("{ID} stt bench failed: {err}"));
    }
}
