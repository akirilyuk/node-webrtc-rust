//! Live AssemblyAI full-stack STT check: the real `VoiceAgent` and its STT actor, not the direct
//! provider calls of `live_test.rs`. Run with `--features live`.
//!
//! AssemblyAI has no TTS, so the audio is the committed Sherpa fixture of the shared phrase. The
//! tests run the generic production VAD preset and the config the runner builds on staging
//! (`runner_staging_config`). Needs `ASSEMBLYAI_API_KEY` (CI maps the repo secret
//! `ASSEMBLYAI_LIVE_KEY`); without it the test skips unless `ASSEMBLYAI_LIVE_REQUIRED=1`. The key
//! and transcripts are never printed.
#![cfg(feature = "live")]

use std::sync::Arc;
use std::time::Duration;

use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;
use node_webrtc_rust_vendor_live_support::audio::STT_WORDS;
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::fullstack::{
    assert_fullstack_contract, run_fullstack, run_fullstack_with_config,
};
use node_webrtc_rust_vendor_live_support::runner_config::runner_staging_config;
use node_webrtc_rust_vendor_live_support::stt_fixture_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "assemblyai";

fn live_key() -> Option<String> {
    live_env(ID, &["ASSEMBLYAI_API_KEY"])
        .map(|mut env| env.remove("ASSEMBLYAI_API_KEY").expect("key"))
}

fn stt(key: String, language: Option<&str>) -> SttConfig {
    SttConfig {
        provider: SttVendor::Assemblyai,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: language.map(str::to_string),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    }
}

fn factory() -> Arc<dyn VendorFactory> {
    Arc::new(AssemblyAiFactory)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemblyai_stt_fullstack_one_final_per_utterance() {
    let Some(key) = live_key() else { return };
    let sample = run_fullstack(
        factory(),
        stt(key, None),
        &stt_fixture_stereo_48k(),
        &STT_WORDS,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} full-stack run failed: {err}"));
    assert_fullstack_contract(ID, &sample, 7, STT_WORDS.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemblyai_stt_fullstack_runner_config_one_final_per_utterance() {
    let Some(key) = live_key() else { return };
    let sample = run_fullstack_with_config(
        factory(),
        runner_staging_config(stt(key, Some("en"))),
        &stt_fixture_stereo_48k(),
        &STT_WORDS,
        Duration::from_secs(4),
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} runner-config full-stack run failed: {err}"));
    assert_fullstack_contract(&format!("{ID}_runner"), &sample, 7, STT_WORDS.len());
}
