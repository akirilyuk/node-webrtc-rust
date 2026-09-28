use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_groq::GroqFactory;

#[test]
fn groq_factory_creates_providers() {
    let factory = GroqFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Groq,
        model: None,
        model_path: None,
        language: Some("en".into()),
        api_key: Some("test-key".into()),
        endpoint: None,
    });
    assert!(stt.is_ok());
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Groq,
        model: None,
        model_path: None,
        voice: Some("troy".into()),
        api_key: Some("test-key".into()),
        endpoint: None,
    });
    assert!(tts.is_ok());
}
