//! Live ElevenLabs full-stack STT check: the real `VoiceAgent` and its STT actor with the production
//! VAD preset (`gate_stt: true`), not the direct provider calls of `live_test.rs`. Run with
//! `--features live`.
//!
//! The counting phrase is synthesized once with the vendor's own TTS and pushed as inbound 20 ms
//! frames. One spoken phrase must produce exactly one `user_speech_final` with at least 8 of the
//! 10 number words. Needs `ELEVENLABS_API_KEY` (CI maps the repo secret `ELEVENLABS_LIVE_KEY`); without it the test skips
//! unless `ELEVENLABS_LIVE_REQUIRED=1`. The key and transcripts are never printed.
#![cfg(feature = "live")]

use std::sync::Arc;
use std::time::Duration;

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use node_webrtc_rust_vendor_live_support::audio::{COUNT_PHRASE, COUNT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::fullstack::{
    assert_fullstack_contract, run_fullstack, run_fullstack_idle_then_speak,
    run_fullstack_with_config,
};
use node_webrtc_rust_vendor_live_support::runner_config::runner_staging_config;
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "elevenlabs";
/// Longer than the ~15 s after which ElevenLabs closes an idle realtime socket.
const IDLE_BEFORE_SPEECH_MS: usize = 20_000;

fn tts(key: &str) -> Box<dyn TtsProvider> {
    ElevenLabsFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Elevenlabs,
            model: voice_catalog::default_tts_model(ID).map(str::to_string),
            model_path: None,
            voice: None,
            api_key: Some(key.to_string()),
            endpoint: None,
            sentence_case: Default::default(),
            transport: Some(SpeechTransport::parse("auto").expect("transport")),
        })
        .unwrap_or_else(|err| panic!("tts config: {err}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();

    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: None,
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        stt,
        &speech,
        &COUNT_WORDS,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} full-stack run failed: {err}"));
    assert_fullstack_contract(ID, &sample, 8, COUNT_WORDS.len());
}

/// The same phrase through the config the runner builds on staging (`runner_staging_config`:
/// language ID early, rnnoise, the runner VAD and barge-in values). ElevenLabs answered such a
/// session with `user_stt_not_found` / `no_partial` after the 4 s listen window (2026-10-19).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_runner_config_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: Some("en".into()),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack_with_config(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        runner_staging_config(stt),
        &speech,
        &COUNT_WORDS,
        Duration::from_secs(4),
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} runner-config full-stack run failed: {err}"));
    assert_fullstack_contract(&format!("{ID}_runner"), &sample, 8, COUNT_WORDS.len());
}

/// The runner config again, with the session idle for 20 s before the caller speaks. ElevenLabs
/// closes a realtime socket that gets no audio for about 15 s (close code 1000, measured
/// 2026-10-19), and the gate sends nothing between utterances. Staging runs ended with
/// `user_stt_not_found` / `no_partial` after 4 s because the audio went to the closed socket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn elevenlabs_stt_fullstack_runner_config_after_idle_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["ELEVENLABS_API_KEY"]) else {
        return;
    };
    let key = env.remove("ELEVENLABS_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();
    let stt = SttConfig {
        provider: SttVendor::Elevenlabs,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: Some("en".into()),
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack_idle_then_speak(
        Arc::new(ElevenLabsFactory) as Arc<dyn VendorFactory>,
        runner_staging_config(stt),
        &speech,
        &COUNT_WORDS,
        Duration::from_secs(4),
        IDLE_BEFORE_SPEECH_MS,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} idle full-stack run failed: {err}"));
    assert_fullstack_contract(&format!("{ID}_idle"), &sample, 8, COUNT_WORDS.len());
}
