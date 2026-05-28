use node_webrtc_rust_speech::config::{TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;

#[test]
fn elevenlabs_factory_creates_tts() {
    let factory = ElevenLabsFactory;
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Elevenlabs,
        model: None,
        model_path: None,
        voice: Some("default".into()),
        api_key: Some("test-key".into()),
    });
    assert!(tts.is_ok());
}
