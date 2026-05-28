use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_openai::OpenAiFactory;

#[test]
fn openai_factory_creates_providers() {
    let factory = OpenAiFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Openai,
        model: Some("whisper-1".into()),
        model_path: None,
        language: Some("en".into()),
        api_key: Some("test-key".into()),
    });
    assert!(stt.is_ok());

    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Openai,
        model: None,
        model_path: None,
        voice: Some("alloy".into()),
        api_key: Some("test-key".into()),
    });
    assert!(tts.is_ok());
}
