use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_cartesia::CartesiaFactory;

#[test]
fn cartesia_factory_creates_tts() {
    let factory = CartesiaFactory;
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Cartesia,
        model: None,
        voice: Some("default".into()),
        api_key: Some("test-key".into()),
    });
    assert!(tts.is_ok());
}
