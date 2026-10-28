//! Real-runtime check that Kokoro and Kitten bundles load and synthesize.
//!
//! Needs `NWR_TEST_SHERPA_KOKORO_DIR` (speaker 3) and `NWR_TEST_SHERPA_KITTEN_DIR` (speaker 1);
//! an unset variable prints `skipped: <VAR> not set` and the leg passes.

use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

const TEXT: &str = "The weather in Berlin is twelve degrees and cloudy.";

async fn check(env_key: &str, sid: &str) {
    let Ok(model_path) = std::env::var(env_key) else {
        println!("skipped: {env_key} not set");
        return;
    };
    let config = TtsConfig {
        provider: TtsVendor::LocalSherpa,
        model: None,
        model_path: Some(model_path),
        voice: Some(sid.into()),
        api_key: None,
        endpoint: None,
        sentence_case: Default::default(),
        transport: None,
    };
    let tts = SherpaFactory.create_tts(&config).expect("create TTS");
    let chunks = tts.synthesize(TEXT).await.expect("synthesis");
    let mut samples: Vec<i16> = Vec::new();
    for chunk in &chunks {
        let (pairs, _) = chunk.pcm.as_chunks::<2>();
        samples.extend(pairs.iter().map(|b| i16::from_le_bytes(*b)));
    }
    assert!(!samples.is_empty(), "{env_key}: empty PCM");
    let duration_ms: u32 = chunks.iter().map(|c| c.duration_ms).sum();
    assert!(
        (1500..=8000).contains(&duration_ms),
        "{env_key}: duration {duration_ms} ms out of range"
    );
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    assert!(peak > 1000, "{env_key}: peak {peak} looks like silence");
    println!("{env_key}: {duration_ms} ms, peak {peak}");
}

#[tokio::test]
async fn kokoro_bundle_synthesizes() {
    check("NWR_TEST_SHERPA_KOKORO_DIR", "3").await;
}

#[tokio::test]
async fn kitten_bundle_synthesizes() {
    check("NWR_TEST_SHERPA_KITTEN_DIR", "1").await;
}
