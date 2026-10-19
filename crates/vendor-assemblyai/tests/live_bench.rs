//! Latency bench against the real API (`vendor-bench.yml`, or locally). Prints one
//! `vendor_bench <json>` line per sample; never a key or a transcript.
//!
//! `NWR_BENCH_RUNS` sets the runs (default 5). Without `ASSEMBLYAI_API_KEY` the bench skips,
//! unless `ASSEMBLYAI_LIVE_REQUIRED=1`. AssemblyAI is STT only; the audio is the committed fixture.
//! Each run gives a `cold` utterance and a `warm` second utterance on the same session.
//!
//! `NWR_BENCH_TRANSPORT` (`all` by default, or a comma list such as `http,websocket`) selects the
//! transports to measure.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, VendorFactory};
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;
use node_webrtc_rust_vendor_live_support::bench::{
    bench_stt_warm, runs_from_env, transports_from_env,
};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt_fixture_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "assemblyai";

fn stt(key: &str) -> Box<dyn SttProvider> {
    AssemblyAiFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Assemblyai,
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
    let Some(mut env) = live_env(ID, &["ASSEMBLYAI_API_KEY"]) else {
        return;
    };
    let key = env.remove("ASSEMBLYAI_API_KEY").expect("key");
    let runs = runs_from_env(5);
    let stt_model = voice_catalog::default_stt_model(ID).expect("default stt model");
    bench_stt_warm(
        ID,
        "websocket",
        stt_model,
        || stt(&key),
        &stt_fixture_stereo_48k(),
        runs,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} stt bench failed: {err}"));
}
