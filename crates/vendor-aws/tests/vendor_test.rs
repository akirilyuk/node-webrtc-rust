use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_aws::AwsFactory;

#[test]
fn aws_factory_creates_providers() {
    let factory = AwsFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Aws,
        model: None,
        model_path: None,
        language: None,
        api_key: None,
        endpoint: None,
    });
    assert!(stt.is_ok());
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Aws,
        model: None,
        model_path: None,
        voice: None,
        api_key: None,
        endpoint: None,
    });
    assert!(tts.is_ok());
}
