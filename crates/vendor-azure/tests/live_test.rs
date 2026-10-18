//! Live Azure speech checks against the real API. Run with `--features live`.
//!
//! Needs `AZURE_SPEECH_KEY`, `AZURE_SPEECH_REGION` (CI maps the repo secrets `AZURE_LIVE_SPEECH_KEY` and `AZURE_LIVE_SPEECH_REGION` to `AZURE_SPEECH_KEY` and `AZURE_SPEECH_REGION`). Without them the tests print a
//! note and return, unless `AZURE_LIVE_REQUIRED=1`, which turns a missing variable into a panic
//! naming it so CI can never go green on a skipped run. Keys and transcripts are never printed.
//!
//! Models are the catalog defaults and the voice is unset (the vendor default), so these tests
//! also guard `contract.cross-vendor-default-leak`.
//!
//! Run with `--test-threads=1`: the tests print timing lines (`azure_*`) that CI copies into the
//! step summary.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_azure::AzureFactory;
use node_webrtc_rust_vendor_live_support::audio::{STT_PHRASE, STT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt::{assert_stt_contract, measure_stt};
use node_webrtc_rust_vendor_live_support::tts::{assert_tts_contract, measure_tts};
use node_webrtc_rust_vendor_live_support::vad_gate::silence_stereo_48k;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "azure";
const TEXT: &str = "Hello from the live check. This reply should stream as it is generated.";

/// The API key; a missing variable skips the test (or panics under `AZURE_LIVE_REQUIRED=1`).
fn live_key() -> Option<String> {
    live_env(ID, &["AZURE_SPEECH_KEY", "AZURE_SPEECH_REGION"])
        .map(|mut env| env.remove("AZURE_SPEECH_KEY").expect("key"))
}

fn tts(key: &str, transport: &str) -> Box<dyn TtsProvider> {
    AzureFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Azure,
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
    AzureFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Azure,
            model: voice_catalog::default_stt_model(ID).map(str::to_string),
            model_path: None,
            language: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            transport: None,
        })
        .unwrap_or_else(|err| panic!("stt config: {err}"))
}

/// A failed synthesis panics with the vendor's own error text (never the key). A 4xx that says
/// terms must be accepted is an account issue (Groq Orpheus), not a code defect.
fn synthesis_failed(what: &str, err: impl std::fmt::Display) -> ! {
    let message = err.to_string();
    let hint = if message.to_ascii_lowercase().contains("terms") {
        " (the vendor wants its terms accepted in the account console: an account issue, not a code defect)"
    } else {
        ""
    };
    panic!("{ID} {what} failed: {message}{hint}")
}

/// One synthesis on the http transport: the streaming contract, and the transport really used.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn azure_tts_live_http() {
    let Some(key) = live_key() else { return };
    let provider = tts(&key, "http");
    let sample = measure_tts(provider.as_ref(), TEXT)
        .await
        .unwrap_or_else(|err| synthesis_failed("http synthesis", err));
    assert_tts_contract(&format!("{ID}_http"), &sample);
    let used = provider.last_transport().expect("last_transport set");
    println!("{ID}_transport transport=http used={used}");
    assert!(used == "h2" || used == "http/1.1", "http used {used}");
}

/// Every transport in the catalog has a live test (needs no key).
#[test]
fn azure_live_tests_cover_every_catalog_transport() {
    for (leg, catalog) in [
        ("tts", voice_catalog::tts_transports(ID)),
        ("stt", voice_catalog::stt_transports(ID)),
    ] {
        // `azure_tts_live_http` and `azure_stt_live_vad_gated_roundtrip` cover http.
        assert_eq!(
            catalog,
            &["http"],
            "{leg}: add a live test for each catalog transport"
        );
    }
}

/// No `voice`: the vendor default voice works (`contract.cross-vendor-default-leak`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn azure_tts_live_without_voice() {
    let Some(key) = live_key() else { return };
    let provider = tts(&key, "auto");
    let sample = measure_tts(provider.as_ref(), TEXT)
        .await
        .unwrap_or_else(|err| synthesis_failed("synthesis without voice", err));
    assert_tts_contract(&format!("{ID}_no_voice"), &sample);
}

/// Speaks a known phrase with the same vendor's TTS, adds silence, gates it with the voice
/// agent's VAD, streams only the speech and checks the words. The transcript is never printed.
/// Groq and Azure transcribe one uploaded file per utterance and return only a final, so the contract asserts zero partials (`emits_partials() == false`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn azure_stt_live_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };
    let speech: Vec<u8> = tts(&key, "auto")
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| synthesis_failed("phrase synthesis", err))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    // 1 s of silence before and 2 s after; the tail is longer than the VAD's 1300 ms hangover,
    // otherwise the speech segment never ends.
    let stereo = [silence_stereo_48k(1000), speech, silence_stereo_48k(2000)].concat();

    let mut provider = stt(&key);
    let sample = measure_stt(provider.as_mut(), &stereo, &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    let _ = provider.stop().await;
    assert!(!emits_partials, "{ID}: unexpected emits_partials()");
    assert_stt_contract(ID, emits_partials, &sample);
}
