use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;

#[test]
fn deepgram_factory_creates_stt_and_tts() {
    let factory = DeepgramFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Deepgram,
        model: None,
        model_path: None,
        language: Some("en".into()),
        api_key: Some("test-key".into()),
        endpoint: None,
    });
    assert!(stt.is_ok());
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Deepgram,
        model: None,
        model_path: None,
        voice: None,
        api_key: Some("test-key".into()),
        endpoint: None,
    });
    assert!(tts.is_ok());
}
