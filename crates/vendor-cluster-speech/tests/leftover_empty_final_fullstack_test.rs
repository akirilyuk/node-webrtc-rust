//! Full-stack leftover empty Final: VoiceAgent + cluster-sherpa + mock speech-service.
//!
//! In-process Zipformer (`local-sherpa`) never emits empty Final (`Ok(None)` when text
//! is empty). The dedicated speech-service path does: remote Zipformer can send
//! `is_final` with empty text, and the cluster vendor also synthesizes `Final("")`
//! on mid-utterance gRPC errors. VAD still runs on the runner, so an empty Final
//! can close STT while the client is still speaking.

mod mock_server;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mock_server::{spawn_mock_speech, MockSpeechState};
use node_webrtc_rust_speech::config::{
    SttVendor, TtsConfig, TtsVendor, VadConfig, VoiceAgentConfig,
};
use node_webrtc_rust_speech::events::SpeechEventKind;
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript};
use node_webrtc_rust_speech::{PcmWriter, VendorRegistry, VoiceAgent};
use node_webrtc_rust_vendor_cluster_speech::{ClusterSherpaFactory, ClusterSherpaStt};
use tokio::time::sleep;

fn loud_stereo_frame() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(3840);
    for _ in 0..960 {
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
        pcm.extend_from_slice(&(i16::MAX / 3).to_le_bytes());
    }
    pcm
}

fn silent_stereo_frame() -> Vec<u8> {
    vec![0_u8; 3840]
}

fn leftover_mock_state() -> MockSpeechState {
    MockSpeechState {
        leftover_empty_final_on_first_audio: Arc::new(AtomicBool::new(true)),
        leftover_empty_emitted: Arc::new(AtomicUsize::new(0)),
        ..MockSpeechState::default()
    }
}

fn stt_cfg(endpoint: &str) -> node_webrtc_rust_speech::SttConfig {
    node_webrtc_rust_speech::SttConfig {
        provider: SttVendor::ClusterSherpa,
        model: None,
        model_path: Some("/models/sherpa/stt/en".into()),
        language: Some("en".into()),
        api_key: Some("test-token".into()),
        endpoint: Some(endpoint.to_string()),
    }
}

/// Dedicated speech-service emits empty `is_final`; cluster-sherpa must surface it
/// (in-process Sherpa swallows empty text as `None`).
#[tokio::test]
async fn leftover_empty_is_final_from_speech_service_surfaces_as_empty_final() {
    let state = leftover_mock_state();
    let leftover_emitted = Arc::clone(&state.leftover_empty_emitted);
    let (url, shutdown) = spawn_mock_speech(state).await.unwrap();
    let mut stt = ClusterSherpaStt::new(&stt_cfg(&url)).unwrap();
    stt.start().await.unwrap();
    sleep(Duration::from_millis(80)).await;
    stt.push_audio(Bytes::from(vec![1_u8; 3200])).await.unwrap();

    let mut saw_empty_final = false;
    for _ in 0..40 {
        if leftover_emitted.load(Ordering::SeqCst) > 0 {
            match stt.poll_transcript().await.unwrap() {
                Some(SttTranscript::Final(text)) if text.trim().is_empty() => {
                    saw_empty_final = true;
                    break;
                }
                _ => {}
            }
        }
        sleep(Duration::from_millis(10)).await;
    }

    stt.stop().await.unwrap();
    let _ = shutdown.send(());
    assert!(
        leftover_emitted.load(Ordering::SeqCst) > 0,
        "mock speech-service must emit leftover empty is_final"
    );
    assert!(
        saw_empty_final,
        "cluster-sherpa must surface empty Final from speech-service (local-sherpa would swallow it)"
    );
}

#[tokio::test]
async fn cluster_leftover_empty_final_while_speaking_does_not_drop_real_utterance() {
    let state = leftover_mock_state();
    let leftover_emitted = Arc::clone(&state.leftover_empty_emitted);
    let (url, shutdown) = spawn_mock_speech(state).await.unwrap();

    let mut registry = VendorRegistry::new();
    let factory = Arc::new(ClusterSherpaFactory);
    registry.register_stt(SttVendor::ClusterSherpa, factory.clone());
    registry.register_tts(TtsVendor::ClusterSherpa, factory);

    let mut vad = VadConfig::default();
    vad.enabled = true;
    vad.threshold = 0.05;
    vad.min_speech_duration_ms = 40;
    vad.min_silence_duration_ms = 40;
    vad.gate_stt = true;
    vad.stt_gate_hold_ms = 80;
    vad.barge_in.enabled = false;

    let config = VoiceAgentConfig {
        stt: Some(stt_cfg(&url)),
        tts: Some(TtsConfig {
            provider: TtsVendor::ClusterSherpa,
            model: None,
            model_path: Some("/models/sherpa/tts/en".into()),
            voice: Some("amy".into()),
            api_key: Some("test-token".into()),
            endpoint: Some(url.clone()),
        }),
        vad,
        ..Default::default()
    };

    let agent = VoiceAgent::new(config, Arc::new(registry)).unwrap();
    let mut events = agent.subscribe_events();
    let writer: PcmWriter = Arc::new(|_pcm, _ms| Ok(()));
    agent.attach(Arc::new(|| Ok(None)), writer).await.unwrap();
    agent.start(None).await.unwrap();
    sleep(Duration::from_millis(80)).await;

    let loud = loud_stereo_frame();
    let silent = silent_stereo_frame();

    let mut leftover_seen = false;
    for _ in 0..40 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
        if leftover_emitted.load(Ordering::SeqCst) > 0 {
            leftover_seen = true;
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    assert!(
        leftover_seen,
        "speech-service must emit leftover empty is_final while VAD is speaking"
    );

    for _ in 0..20 {
        agent
            .process_inbound_pcm(Bytes::from(loud.clone()), 20)
            .await
            .unwrap();
    }
    for _ in 0..30 {
        agent
            .process_inbound_pcm(Bytes::from(silent.clone()), 20)
            .await
            .unwrap();
        sleep(Duration::from_millis(5)).await;
    }

    agent.stop().await.unwrap();
    let _ = shutdown.send(());

    let mut empty_finals = 0_u32;
    let mut nonempty_finals = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.kind == SpeechEventKind::UserSpeechFinal {
            let text = event.text.clone().unwrap_or_default();
            if text.trim().is_empty() {
                empty_finals += 1;
            } else {
                nonempty_finals.push(text);
            }
        }
    }

    assert_eq!(
        empty_finals, 0,
        "empty leftover Final from speech-service while VAD is speaking must not be emitted"
    );
    assert!(
        nonempty_finals.iter().any(|text| text.contains("one")),
        "real utterance must still emit a non-empty user_speech_final, got {nonempty_finals:?}"
    );
}
