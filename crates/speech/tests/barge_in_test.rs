//! Barge-in behavior tests for all three configuration modes.

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use node_webrtc_rust_speech::config::BargeInConfig;
use node_webrtc_rust_speech::events::{SpeechEventKind, SpeechEventBus};
use node_webrtc_rust_speech::pipeline::TtsAudioChunk;
use node_webrtc_rust_speech::tts_buffer::TtsBuffer;
use node_webrtc_rust_speech::vad::handle_barge_in;
use tokio::time::{timeout, Duration};

async fn run_barge_in(barge_in: BargeInConfig) -> (usize, bool) {
    let bus = SpeechEventBus::new();
    let mut rx = bus.subscribe();
    let tts_buffer = TtsBuffer::new();
    tts_buffer
        .enqueue(vec![TtsAudioChunk {
            pcm: Bytes::from_static(b"audio"),
            duration_ms: 20,
        }])
        .await;

    let events: Arc<Mutex<Vec<SpeechEventKind>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);
    handle_barge_in(&barge_in, &tts_buffer, |event| {
        events_clone.lock().unwrap().push(event.kind);
        bus.emit(event);
    })
    .await;

    let pending = tts_buffer.pending_count().await;
    let _ = timeout(Duration::from_millis(100), rx.recv()).await;
    let kinds = events.lock().unwrap().clone();
    (pending, kinds.contains(&SpeechEventKind::BargeIn))
}

#[tokio::test]
async fn barge_in_flush_then_emit() {
    let (pending, saw_barge_in) = run_barge_in(BargeInConfig {
        enabled: true,
        use_vad: true,
        flush_tts: true,
    })
    .await;
    assert_eq!(pending, 0);
    assert!(saw_barge_in);
}

#[tokio::test]
async fn barge_in_notify_only() {
    let (pending, saw_barge_in) = run_barge_in(BargeInConfig {
        enabled: true,
        use_vad: true,
        flush_tts: false,
    })
    .await;
    assert_eq!(pending, 1);
    assert!(saw_barge_in);
}

#[tokio::test]
async fn barge_in_disabled() {
    let (pending, saw_barge_in) = run_barge_in(BargeInConfig {
        enabled: false,
        use_vad: true,
        flush_tts: true,
    })
    .await;
    assert_eq!(pending, 1);
    assert!(!saw_barge_in);
}
