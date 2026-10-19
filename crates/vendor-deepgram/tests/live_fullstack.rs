//! Live Deepgram full-stack STT check: the real `VoiceAgent` and its STT actor with the production
//! VAD preset (`gate_stt: true`), not the direct provider calls of `live_test.rs`. Run with
//! `--features live`.
//!
//! The counting phrase is synthesized once with the vendor's own TTS and pushed as inbound 20 ms
//! frames. One spoken phrase must produce exactly one `user_speech_final` with at least 8 of the
//! 10 number words. Needs `DEEPGRAM_API_KEY` (CI maps the repo secret `DEEPGRAM_LIVE_KEY`); without it the test skips
//! unless `DEEPGRAM_LIVE_REQUIRED=1`. The key and transcripts are never printed.
#![cfg(feature = "live")]

use std::sync::Arc;

use node_webrtc_rust_speech::config::{
    SpeechTransport, SttConfig, SttVendor, TtsConfig, TtsVendor,
};
use node_webrtc_rust_speech::pipeline::{TtsProvider, VendorFactory};
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;
use node_webrtc_rust_vendor_live_support::audio::{COUNT_PHRASE, COUNT_WORDS};
use node_webrtc_rust_vendor_live_support::env::live_env;
use node_webrtc_rust_vendor_live_support::fullstack::{assert_fullstack_contract, run_fullstack};
use node_webrtc_rust_voice_catalog as voice_catalog;

const ID: &str = "deepgram";

fn tts(key: &str) -> Box<dyn TtsProvider> {
    DeepgramFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Deepgram,
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
async fn deepgram_stt_fullstack_one_final_per_utterance() {
    let Some(mut env) = live_env(ID, &["DEEPGRAM_API_KEY"]) else {
        return;
    };
    let key = env.remove("DEEPGRAM_API_KEY").expect("key");
    let speech: Vec<u8> = tts(&key)
        .synthesize_progressive(COUNT_PHRASE, None)
        .await
        .unwrap_or_else(|err| panic!("{ID} phrase synthesis failed: {err}"))
        .into_iter()
        .flat_map(|chunk| chunk.pcm)
        .collect();

    let stt = SttConfig {
        provider: SttVendor::Deepgram,
        model: voice_catalog::default_stt_model(ID).map(str::to_string),
        model_path: None,
        language: None,
        api_key: Some(key),
        endpoint: None,
        transport: None,
    };
    let sample = run_fullstack(
        Arc::new(DeepgramFactory) as Arc<dyn VendorFactory>,
        stt,
        &speech,
        &COUNT_WORDS,
    )
    .await
    .unwrap_or_else(|err| panic!("{ID} full-stack run failed: {err}"));
    assert_fullstack_contract(ID, &sample, 8, COUNT_WORDS.len());
}
