//! Live ElevenLabs TTS and Scribe checks against the real API. Run with `--features live`.
//!
//! Needs `ELEVENLABS_API_KEY` (CI maps the repo secret `ELEVENLABS_LIVE_KEY` to it). Without a key the tests print a
//! note and return, unless `ELEVENLABS_LIVE_REQUIRED=1`, which turns a missing key into a panic so
//! CI can never go green on a skipped run. The key and transcripts are never printed.
//!
//! Models are the catalog defaults and the voice is unset (the vendor default), so these tests
//! also guard `contract.cross-vendor-default-leak`.
//!
//! Run with `--test-threads=1`: the tests print timing lines (`elevenlabs_*`) that CI copies into the
//! step summary.
#![cfg(feature = "live")]

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{SttProvider, TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use node_webrtc_rust_vendor_live_support::audio::{STT_PHRASE, STT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::stt::{assert_stt_contract, measure_stt};
use node_webrtc_rust_vendor_live_support::tts::{assert_tts_contract, measure_tts};
use node_webrtc_rust_vendor_live_support::vad_gate::{silence_stereo_48k, STT_CLIP_TAIL_MS};
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "elevenlabs";
const TEXT: &str = "Hello from the live check. This reply should stream as it is generated.";

fn live_key() -> Option<String> {
    live_env(ID, &["ELEVENLABS_API_KEY"])
        .map(|mut env| env.remove("ELEVENLABS_API_KEY").expect("key"))
}

fn tts(key: &str, transport: &str) -> Box<dyn TtsProvider> {
    ElevenLabsFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
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
    ElevenLabsFactory
        .create_stt(&SttConfig {
            provider: SttVendor::Elevenlabs,
            model: voice_catalog::default_stt_model(ID).map(str::to_string),
            model_path: None,
            language: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            transport: None,
        })
        .unwrap_or_else(|err| panic!("stt config: {err}"))
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
    if transport == "websocket" {
        assert_eq!(used, "ws");
    } else {
        assert!(used == "h2" || used == "http/1.1", "http used {used}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_tts_live_websocket() {
    assert_transport("websocket").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_tts_live_http() {
    assert_transport("http").await;
}

/// Every transport in the catalog has a live test above (needs no key).
#[test]
fn elevenlabs_live_tests_cover_every_catalog_transport() {
    let mut covered = vec!["websocket", "http"];
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
async fn elevenlabs_tts_live_without_voice() {
    let Some(key) = live_key() else { return };
    let provider = tts(&key, "auto");
    let sample = measure_tts(provider.as_ref(), TEXT)
        .await
        .unwrap_or_else(|err| panic!("{ID} synthesis without voice failed: {err}"));
    assert_tts_contract(&format!("{ID}_no_voice"), &sample);
}

/// Speaks a known phrase with the same vendor's TTS, adds silence, gates it with the voice
/// agent's VAD, streams only the speech and checks the words. The transcript is never printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_live_vad_gated_roundtrip() {
    let Some(key) = live_key() else { return };
    let speech: Vec<u8> = tts(&key, "auto")
        .synthesize_progressive(STT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    // 1 s of silence before and 2 s after; the tail is longer than the VAD's 1300 ms hangover,
    // otherwise the speech segment never ends.
    let stereo = [
        silence_stereo_48k(1000),
        speech,
        silence_stereo_48k(STT_CLIP_TAIL_MS),
    ]
    .concat();

    let mut provider = stt(&key);
    let sample = measure_stt(provider.as_mut(), &stereo, &STT_WORDS)
        .await
        .unwrap_or_else(|err| panic!("{ID} stt failed: {err}"));
    let emits_partials = provider.emits_partials();
    let _ = provider.stop().await;
    assert_stt_contract(ID, emits_partials, &sample);
}
