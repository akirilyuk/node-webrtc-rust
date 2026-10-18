//! Live OpenAI speech and transcription checks against the real API. Run with `--features live`.
//!
//! Needs `OPENAI_API_KEY` (CI maps the repo secret `OPENAI_LIVE_KEY` to it). Without a key the
//! tests print a note and return, unless `OPENAI_LIVE_REQUIRED=1`, which turns a missing key
//! into a panic so CI can never go green on a skipped run. The key and transcripts are never
//! printed.
//!
//! Models are the catalog defaults (`whisper-1`, `tts-1`) and the voice is unset, so these
//! tests also guard `contract.cross-vendor-default-leak`. The realtime test uses the catalog's
//! realtime transcription model. `live_openai_matrix.rs` (the `OPENAI_LIVE=1` model matrix) is
//! separate and unchanged.
//!
//! Run with `--test-threads=1`: the tests print timing lines (`openai_*`) that CI copies into
//! the step summary.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_live_support::audio::{STT_PHRASE, STT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt::{assert_stt_contract, measure_stt};
use node_webrtc_rust_vendor_live_support::tts::{assert_tts_contract, measure_tts};
use node_webrtc_rust_vendor_live_support::vad_gate::silence_stereo_48k;
use node_webrtc_rust_vendor_openai::{documented_realtime_model, OpenAiFactory, SttRealtimeMode};
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "openai";
const TEXT: &str = "Hello from the live check. This reply should stream as it is generated.";

fn live_key() -> Option<String> {
    live_env(ID, &["OPENAI_API_KEY"]).map(|mut env| env.remove("OPENAI_API_KEY").expect("key"))
}

fn tts(key: &str, transport: &str) -> Box<dyn TtsProvider> {
    OpenAiFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Openai,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            // The crate falls back to its own default voice; never another vendor's value.
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

/// The phrase synthesized with OpenAI's own TTS, padded with silence (1 s before, 2 s after;
/// the tail is longer than the VAD's 1300 ms hangover, otherwise the segment never ends).
async fn phrase_audio(key: &str) -> Vec<u8> {
    let speech: Vec<u8> = tts(key, "auto")
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    [silence_stereo_48k(1000), speech, silence_stereo_48k(2000)].concat()
}

/// One synthesis per transport: the streaming contract, and the transport that was really used.
async fn assert_transport(transport: &str) {
    let Some(key) = live_key() else { return };
    let provider = tts(&key, transport);
    let sample = measure_tts(provider.as_ref(), TEXT)
        .await
        .unwrap_or_else(|err| panic!("{ID} {transport} synthesis failed: {err}"));
    assert_tts_contract(&format!("{ID}_{transport}"), &sample);
    let used = provider.last_transport().expect("last_transport set");
    println!("{ID}_transport transport={transport} used={used}");
    assert!(
        ["h2", "http/1.1", "sdk"].contains(&used.as_str()),
        "http used {used}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_tts_live_http() {
    assert_transport("http").await;
}

/// Every transport in the catalog has a live test above (needs no key).
#[test]
fn openai_live_tests_cover_every_catalog_transport() {
    let mut covered = vec!["http"];
    let mut catalog = voice_catalog::tts_transports(ID).to_vec();
    covered.sort_unstable();
    catalog.sort_unstable();
    assert_eq!(
        covered, catalog,
        "add a live test for each catalog transport"
    );
}

/// No `voice`: the vendor default voice works (`contract.cross-vendor-default-leak`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_tts_live_without_voice() {
    let Some(key) = live_key() else { return };
    let provider = tts(&key, "auto");
    let sample = measure_tts(provider.as_ref(), TEXT)
        .await
        .unwrap_or_else(|err| panic!("{ID} synthesis without voice failed: {err}"));
    assert_tts_contract(&format!("{ID}_no_voice"), &sample);
}

/// Speaks a known phrase with OpenAI TTS, gates it with the voice agent's VAD, sends only the
/// speech to the default model and checks the words. `whisper-1` uploads the utterance at the
/// end, so it has no partials (`emits_partials()` says so). The transcript is never printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_stt_live_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };
    let stereo = phrase_audio(&key).await;
    let model = voice_catalog::default_stt_model(ID).expect("default stt model");
    let mut provider = stt(&key, model);
    let sample = measure_stt(provider.as_mut(), &stereo, &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    assert!(!emits_partials, "{model} is a file model: finals only");
    let _ = provider.stop().await;
    assert_stt_contract(ID, emits_partials, &sample);
}

/// The same roundtrip on the catalog's realtime transcription model, which streams partials.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_stt_live_realtime_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };
    let model = documented_realtime_model(SttRealtimeMode::Live);
    assert!(
        voice_catalog::stt_models(ID)
            .expect("openai in the catalog")
            .contains(&model),
        "{model} is not in the catalog"
    );
    let stereo = phrase_audio(&key).await;
    let mut provider = stt(&key, model);
    let sample = measure_stt(provider.as_mut(), &stereo, &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} realtime stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    assert!(emits_partials, "{model} is a realtime model: partials");
    let _ = provider.stop().await;
    assert_stt_contract(&format!("{ID}_realtime"), emits_partials, &sample);
}
