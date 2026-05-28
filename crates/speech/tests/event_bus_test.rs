//! Event bus unit tests.

use tokio::time::{timeout, Duration};

use node_webrtc_rust_speech::events::{SpeechEvent, SpeechEventBus, SpeechEventKind};

#[tokio::test]
async fn event_bus_delivers_to_subscriber() {
    let bus = SpeechEventBus::new();
    let mut rx = bus.subscribe();

    bus.emit(SpeechEvent::user_speaking_start());

    let event = timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");

    assert_eq!(event.kind, SpeechEventKind::UserSpeakingStart);
}

#[tokio::test]
async fn event_bus_supports_multiple_subscribers() {
    let bus = SpeechEventBus::new();
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();

    bus.emit(SpeechEvent::barge_in());

    let e1 = rx1.recv().await.unwrap();
    let e2 = rx2.recv().await.unwrap();
    assert_eq!(e1.kind, SpeechEventKind::BargeIn);
    assert_eq!(e2.kind, SpeechEventKind::BargeIn);
}
