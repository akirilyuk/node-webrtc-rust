use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_google::GoogleFactory;

#[test]
fn google_factory_creates_providers() {
    let factory = GoogleFactory;
    assert!(factory
        .create_stt(&SttConfig {
            provider: SttVendor::Google,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        })
        .is_ok());
    assert!(factory
        .create_tts(&TtsConfig {
            provider: TtsVendor::Google,
            model: None,
            voice: Some("en-US-Neural2-A".into()),
            api_key: None,
        })
        .is_ok());
}
