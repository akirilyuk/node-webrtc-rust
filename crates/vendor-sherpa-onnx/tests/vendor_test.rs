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

#[tokio::test]
async fn start_fails_without_model_path_when_env_unset() {
    let _guard = EnvGuard::unset("SHERPA_MODEL_PATH");

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
