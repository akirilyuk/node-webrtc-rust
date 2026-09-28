//! Builds the default vendor registry for VoiceAgent.

use std::sync::Arc;

use node_webrtc_rust_speech::config::{SttVendor, TtsVendor};
use node_webrtc_rust_speech::pipeline::VendorFactory;
use node_webrtc_rust_speech::registry::VendorRegistry;
use node_webrtc_rust_vendor_assemblyai::AssemblyAiFactory;
use node_webrtc_rust_vendor_aws::AwsFactory;
use node_webrtc_rust_vendor_azure::AzureFactory;
use node_webrtc_rust_vendor_cartesia::CartesiaFactory;
use node_webrtc_rust_vendor_deepgram::DeepgramFactory;
use node_webrtc_rust_vendor_elevenlabs::ElevenLabsFactory;
use node_webrtc_rust_vendor_google::GoogleFactory;
use node_webrtc_rust_vendor_groq::GroqFactory;
use node_webrtc_rust_vendor_mock::MockFactory;
use node_webrtc_rust_vendor_openai::OpenAiFactory;
use node_webrtc_rust_vendor_cluster_speech::ClusterSherpaFactory;
use node_webrtc_rust_vendor_sherpa_onnx::SherpaFactory;

fn arc_factory<F: VendorFactory + 'static>(factory: F) -> Arc<dyn VendorFactory> {
    Arc::new(factory)
}

pub fn default_vendor_registry() -> Arc<VendorRegistry> {
    let mut registry = VendorRegistry::new();

    registry.register_stt(SttVendor::Openai, arc_factory(OpenAiFactory));
    registry.register_tts(TtsVendor::Openai, arc_factory(OpenAiFactory));

    registry.register_stt(SttVendor::Deepgram, arc_factory(DeepgramFactory));
    registry.register_tts(TtsVendor::Deepgram, arc_factory(DeepgramFactory));

    registry.register_stt(SttVendor::Elevenlabs, arc_factory(ElevenLabsFactory));
    registry.register_tts(TtsVendor::Elevenlabs, arc_factory(ElevenLabsFactory));

    registry.register_stt(SttVendor::Google, arc_factory(GoogleFactory));
    registry.register_tts(TtsVendor::Google, arc_factory(GoogleFactory));

    registry.register_tts(TtsVendor::Cartesia, arc_factory(CartesiaFactory));

    registry.register_stt(SttVendor::Assemblyai, arc_factory(AssemblyAiFactory));

    registry.register_stt(SttVendor::Groq, arc_factory(GroqFactory));
    registry.register_tts(TtsVendor::Groq, arc_factory(GroqFactory));

    registry.register_stt(SttVendor::Azure, arc_factory(AzureFactory));
    registry.register_tts(TtsVendor::Azure, arc_factory(AzureFactory));

    registry.register_stt(SttVendor::Aws, arc_factory(AwsFactory));
    registry.register_tts(TtsVendor::Aws, arc_factory(AwsFactory));

    registry.register_stt(SttVendor::LocalSherpa, arc_factory(SherpaFactory));
    registry.register_tts(TtsVendor::LocalSherpa, arc_factory(SherpaFactory));

    registry.register_stt(SttVendor::ClusterSherpa, arc_factory(ClusterSherpaFactory));
    registry.register_tts(TtsVendor::ClusterSherpa, arc_factory(ClusterSherpaFactory));

    registry.register_stt(SttVendor::Mock, arc_factory(MockFactory));
    registry.register_tts(TtsVendor::Mock, arc_factory(MockFactory));

    Arc::new(registry)
}
