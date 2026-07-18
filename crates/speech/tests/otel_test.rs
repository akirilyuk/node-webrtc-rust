//! OpenTelemetry voice pipeline hooks (requires `--features otel`).
#![cfg(feature = "otel")]

use node_webrtc_rust_speech::config::{
    SttConfig, SttVendor, TtsConfig, TtsVendor, VoiceAgentConfig, VoiceSessionContext,
};
use node_webrtc_rust_speech::otel::{self, extract_trace_id};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;

fn mock_agent() -> std::sync::Arc<VoiceAgent> {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, std::sync::Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, std::sync::Arc::new(MockFactory));
    let registry = std::sync::Arc::new(registry);
    let config = VoiceAgentConfig {
        stt: Some(SttConfig {
            provider: SttVendor::Mock,
            model: None,
            model_path: None,
            language: Some("en".into()),
            api_key: None,
        }),
        tts: Some(TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        ..Default::default()
    };
    VoiceAgent::new(config, registry).expect("agent")
}

#[test]
fn extract_trace_id_parses_w3c_traceparent() {
    let trace_id = extract_trace_id("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        .expect("trace id");
    assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
}

#[test]
fn init_from_env_is_idempotent() {
    unsafe {
        std::env::set_var("OTEL_SDK_DISABLED", "true");
    }
    otel::init_from_env().expect("init");
    otel::init_from_env().expect("init again");
    unsafe {
        std::env::remove_var("OTEL_SDK_DISABLED");
    }
}

#[test]
fn metrics_record_without_panic() {
    otel::record_stt_latency_ms(12.5, Some(SttVendor::Mock));
    otel::record_tts_latency_ms(34.0, Some(TtsVendor::Mock));
    otel::record_sherpa_pool_wait_ms(2.0);
    otel::set_sherpa_pool_entries(3);
}

#[tokio::test]
async fn start_accepts_session_context_with_traceparent() {
    let agent = mock_agent();
    let pcm_writer: PcmWriter = std::sync::Arc::new(|_pcm, _ms| Ok(()));
    let pcm_reader = std::sync::Arc::new(|| Ok(None));
    agent.attach(pcm_reader, pcm_writer).await.expect("attach");

    let ctx = VoiceSessionContext {
        session_id: Some("sess-1".into()),
        trace_id: Some("4bf92f3577b34da6a3ce929d0e0e4736".into()),
        project_id: Some("proj-1".into()),
        org_id: Some("org-1".into()),
        build_id: Some("build-1".into()),
        traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into()),
    };
    agent.start(Some(ctx)).await.expect("start");
    agent.stop().await.expect("stop");
}
