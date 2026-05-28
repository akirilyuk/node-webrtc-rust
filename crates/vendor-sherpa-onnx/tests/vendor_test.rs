use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn unset(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        // SAFETY: test runs single-threaded for env mutation.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

#[test]
fn sherpa_factory_creates_stt() {
    let factory = SherpaFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::LocalSherpa,
        model: None,
        model_path: None,
        language: Some("en".into()),
        api_key: None,
    });
    assert!(stt.is_ok());
}

#[test]
fn sherpa_factory_creates_tts() {
    let factory = SherpaFactory;
    let tts = factory.create_tts(&node_webrtc_rust_speech::config::TtsConfig {
        provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
        model: None,
        model_path: None,
        voice: Some("0".into()),
        api_key: None,
    });
    assert!(tts.is_ok());
}

#[tokio::test]
async fn tts_synthesize_fails_without_model_path_when_env_unset() {
    let _guard = EnvGuard::unset("SHERPA_TTS_MODEL_PATH");

    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&node_webrtc_rust_speech::config::TtsConfig {
            provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
            model: None,
            model_path: None,
            voice: Some("0".into()),
            api_key: None,
        })
        .expect("factory should create TTS");

    let result = tts.synthesize("hello").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn stt_start_fails_without_model_path_when_env_unset() {
    let _stt = EnvGuard::unset("SHERPA_STT_MODEL_PATH");
    let _legacy = EnvGuard::unset("SHERPA_MODEL_PATH");

    let factory = SherpaFactory;
    let mut stt = factory
        .create_stt(&SttConfig {
            provider: SttVendor::LocalSherpa,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        })
        .expect("factory should create STT");

    let result = stt.start().await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "requires downloaded Piper/VITS bundle; set SHERPA_TTS_MODEL_PATH"]
async fn tts_synthesize_produces_stereo_pcm_with_model() {
    let model_path = std::env::var("SHERPA_TTS_MODEL_PATH")
        .expect("set SHERPA_TTS_MODEL_PATH to a Piper/VITS directory");

    let factory = SherpaFactory;
    let tts = factory
        .create_tts(&node_webrtc_rust_speech::config::TtsConfig {
            provider: node_webrtc_rust_speech::config::TtsVendor::LocalSherpa,
            model: None,
            model_path: Some(model_path),
            voice: Some("0".into()),
            api_key: None,
        })
        .expect("factory should create TTS");

    let chunks = tts
        .synthesize("Hello from local Sherpa TTS.")
        .await
        .expect("synthesis should succeed");

    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert!(chunk.pcm.len() > 3840, "expected more than one 20 ms frame");
    assert_eq!(chunk.pcm.len() % 4, 0, "stereo s16le byte length");
    assert!(chunk.duration_ms >= 100);
    // Opus requires 20 ms (3840 B) or 5 ms (960 B) aligned frames downstream.
    assert_eq!(chunk.pcm.len() % 3840, 0, "PCM should align to 20 ms stereo frames");
}
