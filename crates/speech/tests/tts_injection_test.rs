//! TTS injection tests through VoiceAgent with mock vendor.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::{SttVendor, TtsVendor, VoiceAgentConfig};
use node_webrtc_rust_speech::{PcmWriter, SpeechEventKind, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_mock::MockFactory;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn tts_injection_writes_pcm_to_outbound() {
    let mut registry = VendorRegistry::new();
    registry.register_stt(SttVendor::Mock, Arc::new(MockFactory));
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let config = VoiceAgentConfig {
        stt: None,
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();

    let written: Arc<Mutex<Vec<(Bytes, u32)>>> = Arc::new(Mutex::new(Vec::new()));
    let written_clone = Arc::clone(&written);
    let writer: PcmWriter = Arc::new(move |pcm, ms| {
        written_clone.lock().unwrap().push((pcm, ms));
        Ok(())
    });
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start().await.unwrap();
    agent.send_text_to_tts("hello agent").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();
    agent.stop().await.unwrap();

    let chunks = written.lock().unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks[0].0.len() > 0);
}

#[tokio::test]
async fn tts_injection_emits_agent_speaking_events() {
    let mut registry = VendorRegistry::new();
    registry.register_tts(TtsVendor::Mock, Arc::new(MockFactory));

    let config = VoiceAgentConfig {
        stt: None,
        tts: Some(node_webrtc_rust_speech::TtsConfig {
            provider: TtsVendor::Mock,
            model: None,
            model_path: None,
            voice: None,
            api_key: None,
        }),
        ..Default::default()
    };
    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut rx = agent.subscribe_events();

    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    let reader = Arc::new(|| Ok(None));
    agent.attach(reader, writer).await.unwrap();
    agent.start().await.unwrap();
    agent.send_text_to_tts("hi").await.unwrap();
    agent.wait_tts_playback_idle().await.unwrap();

    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert_eq!(event.kind, SpeechEventKind::AgentSpeakingStart);
}
