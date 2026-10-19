//! Latency bench against the real API (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5). Without `CARTESIA_API_KEY` and `CARTESIA_VOICE_ID`
//! the bench skips, unless `CARTESIA_LIVE_REQUIRED=1`. Cartesia is TTS only.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_cartesia::CartesiaFactory;
use node_webrtc_rust_vendor_live_support::bench::{bench_tts, runs_from_env};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "cartesia";

fn tts(key: &str, voice: &str, transport: &str) -> Box<dyn TtsProvider> {
    CartesiaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Cartesia,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            voice: Some(voice.to_string()),
            api_key: Some(key.to_string()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::parse(transport).expect("transport")),
        })
        .unwrap_or_else(|err| panic!("tts config: {err}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bench() {
    let Some(mut env) = live_env(ID, &["CARTESIA_API_KEY", "CARTESIA_VOICE_ID"]) else {
        return;
    };
    let key = env.remove("CARTESIA_API_KEY").expect("key");
    let voice = env.remove("CARTESIA_VOICE_ID").expect("voice");
    let runs = runs_from_env(5);
    let tts_model = voice_catalog::default_tts_model(ID).expect("default tts model");

    for transport in voice_catalog::tts_transports(ID) {
        bench_tts(
            ID,
            transport,
            tts_model,
            || tts(&key, &voice, transport),
            runs,
        )
        .await
        .unwrap_or_else(|err| panic!("{ID} {transport} tts bench failed: {err}"));
    }
}
