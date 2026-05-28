use node_webrtc_rust_speech::config::{SttConfig, SttVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;

#[test]
fn assemblyai_factory_creates_stt() {
    let factory = AssemblyAiFactory;
    let stt = factory.create_stt(&SttConfig {
        provider: SttVendor::Assemblyai,
        model: None,
        language: Some("en".into()),
        api_key: Some("test-key".into()),
    });
    assert!(stt.is_ok());
}
