//! Live Cartesia TTS checks against the real API. Run with `--features live`.
//!
//! Needs `CARTESIA_API_KEY` and `CARTESIA_VOICE_ID` (CI maps the repo secrets `CARTESIA_LIVE_KEY`
//! and `CARTESIA_LIVE_VOICE_ID` to them). Cartesia has no default voice, so the voice id is
//! required. Without both the tests print a note and return, unless `CARTESIA_LIVE_REQUIRED=1`,
//! which turns a missing variable into a panic so CI can never go green on a skipped run. The key
//! is never printed.
//!
//! Run with `--test-threads=1`: the tests print timing lines (`cartesia_*`) that CI copies into the
//! step summary, and one test edits `CARTESIA_VOICE_ID`.
#![cfg(feature = "live")]

use std::sync::Mutex;

use node_webrtc_rust_speech::config::{SpeechTransport, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::error::SpeechError;
use node_webrtc_rust_speech::pipeline::{TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_cartesia::CartesiaFactory;
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::tts::{assert_tts_contract, measure_tts};
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "cartesia";
const TEXT: &str = "Hello from the live check. This reply should stream as it is generated.";

/// Serializes reads of `CARTESIA_VOICE_ID` against the test that removes it.
static VOICE_ENV: Mutex<()> = Mutex::new(());

fn live_vars() -> Option<(String, String)> {
    let _guard = VOICE_ENV.lock().unwrap_or_else(|e| e.into_inner());
    live_env(ID, &["CARTESIA_API_KEY", "CARTESIA_VOICE_ID"]).map(|mut env| {
        (
            env.remove("CARTESIA_API_KEY").expect("key"),
            env.remove("CARTESIA_VOICE_ID").expect("voice"),
        )
    })
}

fn tts(key: &str, voice: Option<&str>, transport: &str) -> Box<dyn TtsProvider> {
    CartesiaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Cartesia,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            voice: voice.map(str::to_string),
            api_key: Some(key.to_string()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::parse(transport).expect("transport")),
        })
        .unwrap_or_else(|err| panic!("tts config: {err}"))
}

/// One synthesis per transport: the streaming contract, and the transport that was really used.
async fn assert_transport(transport: &str) {
    let Some((key, voice)) = live_vars() else {
        return;
    };
    let provider = tts(&key, Some(&voice), transport);
    let sample = measure_tts(provider.as_ref(), TEXT)
        .await
        .unwrap_or_else(|err| panic!("{ID} {transport} synthesis failed: {err}"));
    assert_tts_contract(&format!("{ID}_{transport}"), &sample);
    let used = provider.last_transport().expect("last_transport set");
    println!("{ID}_transport transport={transport} used={used}");
    if transport == "websocket" {
        assert_eq!(used, "ws");
    } else {
        assert!(used == "h2" || used == "http/1.1", "http used {used}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cartesia_tts_live_websocket() {
    assert_transport("websocket").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cartesia_tts_live_http() {
    assert_transport("http").await;
}

/// Every transport in the catalog has a live test above (needs no key).
#[test]
fn cartesia_live_tests_cover_every_catalog_transport() {
    let mut covered = vec!["websocket", "http"];
    let mut catalog = voice_catalog::tts_transports(ID).to_vec();
    covered.sort_unstable();
    catalog.sort_unstable();
    assert_eq!(
        covered, catalog,
        "add a live test for each catalog transport"
    );
}

/// Two utterances on one provider both meet the streaming contract (the socket is kept open
/// across utterances, each on its own `context_id`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cartesia_tts_live_reuses_socket() {
    let Some((key, voice)) = live_vars() else {
        return;
    };
    let provider = tts(&key, Some(&voice), "websocket");
    for n in 1..=2 {
        let sample = measure_tts(provider.as_ref(), TEXT)
            .await
            .unwrap_or_else(|err| panic!("{ID} utterance {n} failed: {err}"));
        assert_tts_contract(&format!("{ID}_reuse{n}"), &sample);
    }
}

/// No voice anywhere is a config error that names `CARTESIA_VOICE_ID` (no network: it fails
/// before any request). Needs no key.
#[test]
fn cartesia_tts_live_without_voice_is_a_config_error() {
    let _guard = VOICE_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let saved = std::env::var("CARTESIA_VOICE_ID").ok();
    std::env::remove_var("CARTESIA_VOICE_ID");
    let provider = tts("unused-key", None, "auto");
    // A plain runtime keeps the env guard out of any await point.
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(provider.synthesize_progressive("hello", None));
    if let Some(value) = saved {
        std::env::set_var("CARTESIA_VOICE_ID", value);
    }
    match result {
        Err(SpeechError::Config(message)) => {
            assert!(message.contains("CARTESIA_VOICE_ID"), "{message}");
        }
        other => panic!("expected a Config error naming CARTESIA_VOICE_ID, got {other:?}"),
    }
}
