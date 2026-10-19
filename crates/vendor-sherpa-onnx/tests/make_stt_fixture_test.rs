//! Generator for `crates/vendor-live-support/fixtures/quick-brown-fox-16k-mono.pcm`, the STT
//! fixture of the live vendor tests: 16 kHz mono s16le of "The quick brown fox jumps over the
//! lazy dog." rendered by local Sherpa `vits-piper-en_US-amy-low`.
//!
//! `#[ignore]`: needs the Piper bundle. Run it through
//! `crates/vendor-live-support/scripts/make-fixture.sh`, which sets the model path and output.
//! Piper noise differs per render, so regenerating changes the bytes; the fixture is checked in
//! and only regenerated on purpose.

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pcm::{i16_samples_to_bytes, stereo_48k_to_mono_16k};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

const PHRASE: &str = "The quick brown fox jumps over the lazy dog.";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "generator: needs SHERPA_TTS_MODEL_PATH (vits-piper-en_US-amy-low) and FIXTURE_OUT"]
async fn write_quick_brown_fox_fixture() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH").expect("set SHERPA_TTS_MODEL_PATH");
    let out = std::env::var("FIXTURE_OUT").expect("set FIXTURE_OUT");
    let tts = SherpaFactory
        .create_tts(&TtsConfig {
            provider: TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(model_path),
            voice: Some("0".into()),
            api_key: None,
            endpoint: None,
            sentence_case: Default::default(),
            transport: None,
        })
        .expect("create TTS");
    let chunks = tts.synthesize(PHRASE).await.expect("synthesize");
    let stereo_48k: Vec<u8> = chunks.iter().flat_map(|c| c.pcm.to_vec()).collect();
    let mono = i16_samples_to_bytes(&stereo_48k_to_mono_16k(&stereo_48k));
    assert!(
        mono.len() > 16_000 * 2,
        "phrase is shorter than 1 s: {} bytes",
        mono.len()
    );
    std::fs::write(&out, &mono).expect("write fixture");
    println!(
        "wrote {} bytes ({} ms) to {out}",
        mono.len(),
        mono.len() / 32
    );
}
