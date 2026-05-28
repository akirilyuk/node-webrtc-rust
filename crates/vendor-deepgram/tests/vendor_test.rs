use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;

#[test]
fn deepgram_factory_creates_stt() {
    let factory = DeepgramFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Deepgram,
        model: None,
        model_path: None,
        language: Some("en".into()),
        api_key: Some("test-key".into()),
    });
    assert!(stt.is_ok());
}
