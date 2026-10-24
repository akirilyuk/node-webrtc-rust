//! The inbound audio stage of a `RemoteTrack` over real loopback WebRTC.
//!
//! A sender peer writes frame-index-marked frames ([`support::frame_marker`]); the receiving
//! peer reads them through `RemoteTrack::read_sample`. Impairment is injected with the seeded
//! test hook (`set_test_impairment`), never the process env, and no assertion depends on
//! wall-clock latency. Tests share one lock because parallel ICE on loopback flakes.

#[path = "support/frame_marker.rs"]
mod frame_marker;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use frame_marker::{analyze, detect_frame, Detected, MarkerSource, SequenceReport};
use node_webrtc_rust_core::inbound_audio::ImpairmentConfig;
use node_webrtc_rust_core::{
    ConnectionState, LocalAudioTrack, PeerConnection, PeerConnectionConfig, RemoteTrack, SdpType,
    SessionDescription,
};
use tokio::sync::Mutex;
use tokio::time::{sleep, sleep_until, timeout, Instant};

const FRAME: Duration = Duration::from_millis(20);
const IMPAIRMENT: &str =
    "loss=0.05,burst=3,burst_rate=0.01,reorder=0.03,reorder_depth=2,dup=0.02,seed=7";

static SERIAL: Mutex<()> = Mutex::const_new(());

fn peer_config() -> PeerConnectionConfig {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("WEBRTC_INCLUDE_LOOPBACK_CANDIDATES", "1"));
    PeerConnectionConfig {
        ice_servers: vec![],
        ..Default::default()
    }
}

async fn wait_connected(pc: &PeerConnection) {
    timeout(Duration::from_secs(30), async {
        loop {
            match pc.connection_state() {
                ConnectionState::Connected => return,
                ConnectionState::Failed | ConnectionState::Closed => {
                    panic!("connection failed: {:?}", pc.connection_state())
                }
                _ => sleep(Duration::from_millis(50)).await,
            }
        }
    })
    .await
    .expect("timed out waiting for connection");
}

struct Pair {
    sender: PeerConnection,
    receiver: PeerConnection,
    track: LocalAudioTrack,
    remote: RemoteTrack,
}

/// Connects a sender (offers, sends one audio track) and a receiver, and returns the
/// receiver's remote track before anything has been read from it.
async fn connect() -> Pair {
    let sender = PeerConnection::new(peer_config()).await.expect("sender pc");
    let receiver = PeerConnection::new(peer_config())
        .await
        .expect("receiver pc");
    let mut events = receiver.subscribe_events();
    let track = LocalAudioTrack::new("marker-audio", "marker-stream");
    sender
        .add_track(track.as_track_local())
        .await
        .expect("add track");

    let offer = sender.create_offer(None).await.expect("offer");
    sender
        .set_local_description(offer)
        .await
        .expect("local offer");
    sender.gathering_complete().await;
    let local = sender.local_description().await.expect("local offer");
    receiver
        .set_remote_description(SessionDescription {
            sdp_type: SdpType::Offer,
            sdp: local.sdp,
        })
        .await
        .expect("remote offer");
    let answer = receiver.create_answer(None).await.expect("answer");
    receiver
        .set_local_description(answer)
        .await
        .expect("local answer");
    receiver.gathering_complete().await;
    let answer = receiver.local_description().await.expect("local answer");
    sender
        .set_remote_description(SessionDescription {
            sdp_type: SdpType::Answer,
            sdp: answer.sdp,
        })
        .await
        .expect("remote answer");
    wait_connected(&sender).await;
    wait_connected(&receiver).await;

    // Kick RTP so the receiver's on_track fires.
    let remote = timeout(Duration::from_secs(15), async {
        loop {
            track
                .write_sample(Bytes::from(vec![0u8; 960]), Duration::from_millis(5))
                .await
                .expect("kick");
            tokio::select! {
                r = events.tracks.recv() => return r.expect("remote track"),
                _ = sleep(Duration::from_millis(30)) => {}
            }
        }
    })
    .await
    .expect("timed out waiting for the remote track");
    Pair {
        sender,
        receiver,
        track,
        remote,
    }
}

/// Reads `remote` into a shared list of detected 20 ms frames until aborted.
fn spawn_reader(remote: RemoteTrack) -> (Arc<Mutex<Vec<Detected>>>, tokio::task::JoinHandle<()>) {
    let received = Arc::new(Mutex::new(Vec::<Detected>::new()));
    let sink = Arc::clone(&received);
    let handle = tokio::spawn(async move {
        while let Ok(sample) = remote.read_sample().await {
            for frame in sample.pcm.chunks(3_840) {
                sink.lock().await.push(detect_frame(frame));
            }
        }
    });
    (received, handle)
}

/// Sends `frames` marked frames paced 20 ms from one anchor, then lets the receive side drain.
async fn send_paced(track: &LocalAudioTrack, frames: u64) {
    let mut marker = MarkerSource::new();
    let start = Instant::now() + Duration::from_millis(100);
    for i in 0..frames {
        sleep_until(start + FRAME * i as u32).await;
        track
            .write_sample(marker.frame(i).pcm, FRAME)
            .await
            .expect("send");
    }
    sleep(Duration::from_millis(1_000)).await;
}

async fn finish(pair: Pair, reader: tokio::task::JoinHandle<()>) {
    reader.abort();
    let _ = pair.sender.close().await;
    let _ = pair.receiver.close().await;
    sleep(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impaired_inbound_track_is_ordered_and_concealed() {
    let _serial = SERIAL.lock().await;
    let pair = connect().await;
    pair.remote
        .set_test_impairment(Some(ImpairmentConfig::parse(IMPAIRMENT).expect("parse")));
    let (received, reader) = spawn_reader(pair.remote.clone());
    send_paced(&pair.track, 150).await;

    let frames = received.lock().await.clone();
    let report: SequenceReport = analyze(&frames);
    let stats = pair.remote.inbound_stats();
    eprintln!("[inbound-stage impaired] {report:?} stats={stats:?}");

    // The stage emits in sequence order: a marker index never goes backwards.
    assert_eq!(report.reordered, 0, "{report:?} {stats:?}");
    // A repeated index is only allowed where the stage counted concealed audio.
    let concealed_frames = stats.concealed_samples / 960;
    assert!(
        report.duplicates + report.trailing_duplicates <= concealed_frames,
        "duplicates not explained by concealment: {report:?} {stats:?}"
    );
    assert!(stats.lost > 0, "{stats:?}");
    assert!(stats.packets_discarded > 0, "{stats:?}");
    assert!(stats.concealment_events > 0, "{stats:?}");

    let json = pair.receiver.get_stats_json().await.expect("stats json");
    let report: serde_json::Value = serde_json::from_str(&json).expect("stats parse");
    let inbound = report
        .as_object()
        .expect("stats object")
        .values()
        .find(|e| e["type"] == "inbound-rtp")
        .expect("inbound-rtp entry");
    assert!(
        inbound["nwrLostPackets"].as_u64().unwrap_or(0) > 0,
        "{inbound}"
    );
    assert!(inbound["packetsDiscarded"].as_u64().is_some(), "{inbound}");
    finish(pair, reader).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_stage_reads_in_arrival_order() {
    let _serial = SERIAL.lock().await;
    let pair = connect().await;
    pair.remote.set_inbound_reorder_enabled(false);
    let (received, reader) = spawn_reader(pair.remote.clone());
    send_paced(&pair.track, 50).await;

    let frames = received.lock().await.clone();
    let report = analyze(&frames);
    eprintln!("[inbound-stage disabled] {report:?}");
    assert_eq!(report.reordered, 0, "{report:?}");
    assert_eq!(report.duplicates, 0, "{report:?}");
    assert_eq!(report.gaps, 0, "{report:?}");
    assert!(report.played >= 45, "{report:?}");
    // The stage never ran.
    assert_eq!(pair.remote.inbound_stats().packets_received, 0);
    finish(pair, reader).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clean_link_adds_no_wait() {
    let _serial = SERIAL.lock().await;
    let pair = connect().await;
    let (received, reader) = spawn_reader(pair.remote.clone());
    send_paced(&pair.track, 100).await;

    let report = analyze(&received.lock().await.clone());
    let stats = pair.remote.inbound_stats();
    eprintln!("[inbound-stage clean] {report:?} stats={stats:?}");
    assert_eq!(stats.lost, 0, "{stats:?}");
    assert_eq!(stats.reordered, 0, "{stats:?}");
    assert_eq!(stats.current_wait_ms, 20, "{stats:?}");
    assert!(report.played >= 90, "{report:?}");
    finish(pair, reader).await;
}
