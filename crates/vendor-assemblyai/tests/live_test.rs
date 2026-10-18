//! Live AssemblyAI streaming checks against the real API. Run with `--features live`.
//!
//! Needs `ASSEMBLYAI_API_KEY` (CI maps the repo secret `ASSEMBLYAI_LIVE_KEY` to it). Without a key
//! the tests print a note and return, unless `ASSEMBLYAI_LIVE_REQUIRED=1`, which turns a missing
//! key into a panic so CI can never go green on a skipped run. The key and transcripts are never
//! printed.
//!
//! AssemblyAI has no TTS, so the audio is the committed Sherpa fixture of the shared phrase
//! (`vendor_live_support::stt_fixture_stereo_48k`). The model is the catalog default.
//!
//! Run with `--test-threads=1`: the tests print timing lines (`assemblyai_*`) that CI copies into
//! the step summary.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, VendorFactory};
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;
use node_webrtc_rust_vendor_live_support::audio::STT_WORDS;
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt::{
    assert_stt_contract, measure_stt, open_stt, run_utterance,
};
use node_webrtc_rust_vendor_live_support::stt_fixture_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "assemblyai";

fn live_key() -> Option<String> {
    live_env(ID, &["ASSEMBLYAI_API_KEY"])
        .map(|mut env| env.remove("ASSEMBLYAI_API_KEY").expect("key"))
}

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

/// Streams the fixture phrase (VAD-gated, paced at real time), finalizes it and checks the words.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemblyai_stt_live_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };
    let mut provider = stt(&key);
    let sample = measure_stt(provider.as_mut(), &stt_fixture_stereo_48k(), &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    let used = provider.last_transport();
    let _ = provider.stop().await;
    assert_eq!(used.as_deref(), Some("ws"));
    assert_stt_contract(ID, emits_partials, &sample);
}

/// Several turns per session are documented, so a second utterance on the open session meets the
/// same contract (the first one ended with `ForceEndpoint`, not `Terminate`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemblyai_stt_live_second_utterance_on_one_session() {
    let Some(key) = live_key() else { return };
    let mut provider = stt(&key);
    let audio = stt_fixture_stereo_48k();
    open_stt(provider.as_mut())
        .await
        .unwrap_or_else(|err| panic!("{ID} open failed: {err}"));
    let emits_partials = provider.emits_partials();
    for n in 1..=2 {
        let sample = run_utterance(provider.as_mut(), &audio, &STT_WORDS)
            .await
            .unwrap_or_else(|err| panic!("{ID} utterance {n} failed: {err}"));
        assert_stt_contract(&format!("{ID}_utt{n}"), emits_partials, &sample);
    }
    let _ = provider.stop().await;
}
