//! Integration tests for cluster-sherpa gRPC vendor (mock tonic server, no ONNX).

mod mock_server;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use mock_server::{spawn_mock_speech, MockSpeechState};
use node_webrtc_rust_speech::config::{SttConfig, SttVendor, TtsConfig, TtsVendor};
use node_webrtc_rust_speech::pipeline::{SttProvider, SttTranscript, TtsProvider, TtsProgressiveSink};
use node_webrtc_rust_vendor_cluster_speech::{
    inc_stt_reopen, stt_reopen_total, ClusterSherpaStt, ClusterSherpaTts,
};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

fn stt_cfg(endpoint: &str) -> SttConfig {
    SttConfig {
        provider: SttVendor::ClusterSherpa,
        model: None,
        model_path: Some("/models/sherpa/stt/en".into()),
        language: Some("en".into()),
        api_key: Some("test-token".into()),
        endpoint: Some(endpoint.to_string()),
    }
}

fn tts_cfg(endpoint: &str) -> TtsConfig {
    TtsConfig {
        provider: TtsVendor::ClusterSherpa,
        model: None,
        model_path: Some("/models/sherpa/tts/en".into()),
        voice: Some("amy".into()),
        api_key: Some("test-token".into()),
        endpoint: Some(endpoint.to_string()),
    }
}

#[tokio::test]
async fn stt_start_ready_finalize_blocks_until_finalized() {
    let state = MockSpeechState::default();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = ClusterSherpaStt::new(&stt_cfg(&url)).unwrap();
    stt.start().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    stt.push_audio(Bytes::from(vec![1_u8; 3200])).await.unwrap();
    stt.finalize_utterance().await.unwrap();
    let mut saw_final = false;
    for _ in 0..20 {
        match stt.poll_transcript().await.unwrap() {
            Some(SttTranscript::Final(_)) => {
                saw_final = true;
                break;
            }
            _ => sleep(Duration::from_millis(10)).await,
        }
    }
    assert!(saw_final);
    assert_eq!(state.finalize_count.load(Ordering::SeqCst), 1);
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test]
async fn two_endpoints_hit_distinct_mock_servers() {
    let state_a = MockSpeechState::default();
    let state_b = MockSpeechState::default();
    let (url_a, shutdown_a) = spawn_mock_speech(state_a.clone()).await.unwrap();
    let (url_b, shutdown_b) = spawn_mock_speech(state_b.clone()).await.unwrap();

    let mut stt_a = ClusterSherpaStt::new(&stt_cfg(&url_a)).unwrap();
    let mut stt_b = ClusterSherpaStt::new(&stt_cfg(&url_b)).unwrap();
    stt_a.start().await.unwrap();
    stt_b.start().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    assert_eq!(state_a.transcribe_streams.load(Ordering::SeqCst), 1);
    assert_eq!(state_b.transcribe_streams.load(Ordering::SeqCst), 1);
    stt_a.stop().await.unwrap();
    stt_b.stop().await.unwrap();
    let _ = shutdown_a.send(());
    let _ = shutdown_b.send(());
}

#[tokio::test]
async fn relocate_after_finalized_reopens_transcribe() {
    node_webrtc_rust_vendor_cluster_speech::reset_stt_reopen_metrics();
    let state = MockSpeechState {
        relocate_after_finalize: Arc::new(AtomicBool::new(true)),
        ..MockSpeechState::default()
    };
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = ClusterSherpaStt::new(&stt_cfg(&url)).unwrap();
    stt.start().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    stt.push_audio(Bytes::from(vec![1_u8; 3200])).await.unwrap();
    stt.finalize_utterance().await.unwrap();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(state.transcribe_streams.load(Ordering::SeqCst), 2);
    assert!(stt_reopen_total("relocate") >= 1);
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test]
async fn goaway_after_finalize_reopens_on_next_finalize() {
    node_webrtc_rust_vendor_cluster_speech::reset_stt_reopen_metrics();
    let state = MockSpeechState {
        close_after_finalize: Arc::new(AtomicBool::new(true)),
        ..MockSpeechState::default()
    };
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = ClusterSherpaStt::new(&stt_cfg(&url)).unwrap();
    stt.start().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    stt.push_audio(Bytes::from(vec![1_u8; 3200])).await.unwrap();
    stt.finalize_utterance().await.unwrap();
    sleep(Duration::from_millis(150)).await;
    stt.push_audio(Bytes::from(vec![2_u8; 3200])).await.unwrap();
    stt.finalize_utterance().await.unwrap();
    assert!(state.transcribe_streams.load(Ordering::SeqCst) >= 2);
    assert!(stt_reopen_total("goaway") >= 1);
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

#[tokio::test]
async fn tts_progressive_emits_chunks_and_honors_cancel() {
    let state = MockSpeechState::default();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let tts = ClusterSherpaTts::new(&tts_cfg(&url)).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, _rx) = mpsc::unbounded_channel();
    let sink = TtsProgressiveSink {
        tx,
        cancel: Arc::clone(&cancel),
    };
    cancel.store(true, Ordering::SeqCst);
    let chunks = tts
        .synthesize_progressive("hello", Some(sink))
        .await
        .unwrap();
    assert!(!chunks.is_empty() || state.synthesize_calls.load(Ordering::SeqCst) == 1);
    let _ = shutdown.send(());
}

#[tokio::test]
async fn endpoint_precedence_env_fallback() {
    let state = MockSpeechState::default();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    unsafe {
        std::env::set_var("SPEECH_STT_SERVICE_URL", &url);
    }
    let cfg = SttConfig {
        provider: SttVendor::ClusterSherpa,
        model: None,
        model_path: Some("/models/sherpa/stt/en".into()),
        language: Some("en".into()),
        api_key: Some("t".into()),
        endpoint: None,
    };
    let mut stt = ClusterSherpaStt::new(&cfg).unwrap();
    stt.start().await.unwrap();
    sleep(Duration::from_millis(50)).await;
    assert_eq!(state.transcribe_streams.load(Ordering::SeqCst), 1);
    stt.stop().await.unwrap();
    unsafe {
        std::env::remove_var("SPEECH_STT_SERVICE_URL");
    }
    let _ = shutdown.send(());
}

#[test]
fn stt_reopen_metric_increments() {
    node_webrtc_rust_vendor_cluster_speech::reset_stt_reopen_metrics();
    inc_stt_reopen("relocate");
    assert_eq!(stt_reopen_total("relocate"), 1);
}

#[tokio::test]
async fn stt_wait_ready_resolves_once_stream_is_open() {
    let state = MockSpeechState::default();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let mut stt = ClusterSherpaStt::new(&stt_cfg(&url)).unwrap();
    // Not started: no stream, never ready.
    assert!(!stt.wait_ready(Duration::from_millis(20)).await.unwrap());
    stt.start().await.unwrap();
    assert!(stt.wait_ready(Duration::from_secs(5)).await.unwrap());
    stt.stop().await.unwrap();
    let _ = shutdown.send(());
}

async fn assert_single_message_intact(n: usize) {
    let state = MockSpeechState {
        synthesize_single_message_bytes: Arc::new(AtomicUsize::new(n)),
        ..MockSpeechState::default()
    };
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    let tts = ClusterSherpaTts::new(&tts_cfg(&url)).unwrap();
    let chunks = tts
        .synthesize("large single message")
        .await
        .unwrap_or_else(|e| panic!("synthesize with {n}-byte single message failed: {e}"));
    let mut pcm = Vec::new();
    for chunk in &chunks {
        pcm.extend_from_slice(chunk.pcm.as_ref());
    }
    assert_eq!(pcm.len(), n, "received PCM length");
    let expected: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
    assert!(
        pcm == expected,
        "received PCM bytes differ from the pattern"
    );
    let _ = shutdown.send(());
}

#[tokio::test]
async fn tts_receives_1mib_single_message_intact() {
    assert_single_message_intact(1_048_576).await;
}

#[tokio::test]
async fn tts_receives_4mb_single_message_intact() {
    // Just under tonic's 4 MiB default decode limit.
    assert_single_message_intact(4_000_000).await;
}

#[tokio::test]
async fn cluster_stt_drop_without_stop_closes_streams() {
    let state = MockSpeechState::default();
    let (url, shutdown) = spawn_mock_speech(state.clone()).await.unwrap();
    for cycle in 0..300 {
        let mut stt = ClusterSherpaStt::new(&stt_cfg(&url)).unwrap();
        stt.start().await.unwrap();
        assert!(
            stt.wait_ready(Duration::from_secs(5)).await.unwrap(),
            "cycle {cycle}: stream never became ready"
        );
        for _ in 0..10 {
            stt.push_audio(Bytes::from(vec![0_u8; 1920])).await.unwrap();
        }
        drop(stt);
    }
    let mut open = state.open_transcribe_streams.load(Ordering::SeqCst);
    for _ in 0..500 {
        if open == 0 {
            break;
        }
        sleep(Duration::from_millis(10)).await;
        open = state.open_transcribe_streams.load(Ordering::SeqCst);
    }
    assert_eq!(
        open, 0,
        "server-side transcribe streams still open after 300 drop-without-stop cycles: {open}"
    );
    let _ = shutdown.send(());
}
