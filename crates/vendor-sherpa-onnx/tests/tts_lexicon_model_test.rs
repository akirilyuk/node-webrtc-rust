//! Real-model check that lexicon VITS (Melo) and Piper bundles both load and synthesize.
//!
//! `#[ignore]`: needs `SHERPA_TTS_MELO_MODEL_PATH` / `SHERPA_TTS_PIPER_MODEL_PATH`.

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

fn tts_config(model_path: String) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some("0".into()),
        api_key: None,
        endpoint: None,
    }
}

async fn synthesize_pcm_bytes(env_key: &str) -> usize {
    let model_path = std::env::var(env_key).unwrap_or_else(|_| panic!("set {env_key}"));
    let tts = SherpaFactory
        .create_tts(&tts_config(model_path))
        .expect("create TTS");
    let chunks = tts
        .synthesize("你好，欢迎使用。Hello.")
        .await
        .expect("synthesis");
    let bytes: usize = chunks.iter().map(|chunk| chunk.pcm.len()).sum();
    assert!(bytes > 0, "expected non-empty PCM from {env_key}");
    // 16-bit mono PCM: two bytes per sample.
    println!("{env_key}: {} samples ({bytes} bytes)", bytes / 2);
    bytes
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_MELO_MODEL_PATH (Melo lexicon bundle)"]
async fn tts_lexicon_model_synthesizes() {
    synthesize_pcm_bytes("SHERPA_TTS_MELO_MODEL_PATH").await;
}

#[tokio::test]
#[ignore = "requires SHERPA_TTS_PIPER_MODEL_PATH (Piper regression bundle)"]
async fn tts_piper_model_still_synthesizes() {
    synthesize_pcm_bytes("SHERPA_TTS_PIPER_MODEL_PATH").await;
}
