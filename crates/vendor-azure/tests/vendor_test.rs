use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_azure::AzureFactory;

#[test]
fn azure_factory_creates_providers() {
    let factory = AzureFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Azure,
        model: None,
        model_path: None,
        language: Some("en-US".into()),
        api_key: Some("test-key".into()),
        endpoint: Some("res.cognitiveservices.azure.com".into()),
    });
    assert!(stt.is_ok());
    let tts = factory.create_tts(&TtsConfig {
        provider: TtsVendor::Azure,
        model: None,
        model_path: None,
        voice: None,
        api_key: Some("test-key".into()),
        endpoint: Some("eastus.tts.speech.microsoft.com".into()),
    });
    assert!(tts.is_ok());
}
