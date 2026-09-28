use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;

#[test]
fn elevenlabs_factory_creates_stt_and_tts() {
    let factory = ElevenLabsFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Elevenlabs,
        model: None,
        model_path: None,
        language: Some("en".into()),
        api_key: Some("test-key".into()),
        endpoint: None,
    });
    assert!(stt.is_ok());
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Elevenlabs,
        model: None,
        model_path: None,
        voice: Some("default".into()),
        api_key: Some("test-key".into()),
        endpoint: None,
    });
    assert!(tts.is_ok());
}
