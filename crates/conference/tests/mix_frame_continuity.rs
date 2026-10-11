//! End-to-end audio continuity through the conference mix over real WebRTC.
//!
//! A speaker peer sends 150 frame-index-marked frames (3 s) to the room; a listener peer
//! decodes the room's personalised mix. Both peers connect to the room over loopback ICE.
//! The listener's frames are classified with the marker detector
//! ([`support::frame_marker`]) and checked for gaps and duplicates.
//!
//! Four send patterns: (a) paced 20 ms, (b) bunched 3 frames every 60 ms, (c) seeded
//! jitter of +/-30 ms, (d) one 60 ms Opus packet (3 frames) every 60 ms. Each runs in its own test; a process-wide lock serialises them
//! because parallel ICE on loopback flakes.

#[path = "support/frame_marker.rs"]
mod frame_marker;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use frame_marker::{analyze, detect_frame, Detected, MarkerSource, SequenceReport};
use node_webrtc_rust_conference::{
    ConferenceServer, Room, RoomConfig, SignalingMessage, SignalingResponse,
};
use node_webrtc_rust_core::{
    ConnectionState, LocalAudioTrack, OfferOptions, PeerConnection, PeerConnectionConfig, SdpType,
    SessionDescription,
};
use node_webrtc_rust_mixer::SourceStats;
use tokio::sync::Mutex;
use tokio::time::{sleep, sleep_until, timeout, Instant};

const FRAMES: u64 = 150;
const FRAME: Duration = Duration::from_millis(20);

static SERIAL: Mutex<()> = Mutex::const_new(());

fn peer_config() -> PeerConnectionConfig {
    // Loopback host candidates only, no STUN (a STUN lookup can hang gathering; see core tests).
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var("WEBRTC_INCLUDE_LOOPBACK_CANDIDATES", "1");
        // The default 400 kbps makes a 60 ms packet ~3 kB, above the RTP MTU, and the sender
        // drops it. 128 kbps keeps a 60 ms packet near 1 kB.
        std::env::set_var("WEBRTC_OPUS_BITRATE_BPS", "128000");
    });
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

/// Adds participant `id` to the room and connects a client peer that offers (as a browser
/// would): the client sends `send_track` when given and always receives the room's mix.
async fn join_client(
    room: &Arc<Mutex<Room>>,
    id: &str,
    send_track: Option<&LocalAudioTrack>,
) -> PeerConnection {
    room.lock()
        .await
        .add_participant(id)
        .await
        .expect("add participant");

    let client = PeerConnection::new(peer_config()).await.expect("client pc");
    if let Some(track) = send_track {
        client
            .add_track(track.as_track_local())
            .await
            .expect("add track");
    }
    let offer = client
        .create_offer(Some(OfferOptions {
            offer_to_receive_audio: true,
            ..Default::default()
        }))
        .await
        .expect("offer");
    client
        .set_local_description(offer)
        .await
        .expect("set local offer");
    client.gathering_complete().await;
    let local = client.local_description().await.expect("local offer");

    let responses = room
        .lock()
        .await
        .handle_signaling(SignalingMessage::Offer {
            participant_id: id.into(),
            sdp: local.sdp,
        })
        .await
        .expect("room answers offer");
    let Some(SignalingResponse::Answer { sdp, .. }) = responses.into_iter().next() else {
        panic!("expected answer");
    };
    client
        .set_remote_description(SessionDescription {
            sdp_type: SdpType::Answer,
            sdp,
        })
        .await
        .expect("set remote answer");
    client
}

#[derive(Clone, Copy, Debug)]
enum Pattern {
    Paced,
    Bunched,
    Jittered,
    /// One 60 ms Opus packet (three frames) per write.
    Packet60,
}

/// Send offsets (ms from start) for frame `i`; every frame is sent no earlier than its
/// predecessor (FIFO), only delayed.
fn send_schedule(pattern: Pattern) -> Vec<u64> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut out: Vec<u64> = Vec::new();
    for i in 0..FRAMES {
        let t = match pattern {
            Pattern::Paced => i * 20,
            Pattern::Bunched | Pattern::Packet60 => (i / 3) * 60,
            Pattern::Jittered => {
                let jitter = (next() % 61) as i64 - 30;
                (i as i64 * 20 + 30 + jitter) as u64
            }
        };
        out.push(t.max(out.last().copied().unwrap_or(0)));
    }
    out
}

struct Outcome {
    report: SequenceReport,
    stats: Option<SourceStats>,
}

async fn run_pattern(pattern: Pattern) -> Outcome {
    let _serial = SERIAL.lock().await;
    // The loopback-candidate switch must be set before the room creates its first peer.
    let _ = peer_config();

    let server = ConferenceServer::new();
    let room = server
        .create_room(
            "continuity",
            RoomConfig {
                noise_suppression: false,
                ..Default::default()
            },
        )
        .await
        .expect("create room");

    let speaker_track = LocalAudioTrack::new("speaker-audio", "speaker-stream");
    let speaker = join_client(&room, "speaker", Some(&speaker_track)).await;
    let listener = join_client(&room, "listener", None).await;
    let mut listener_events = listener.subscribe_events();
    wait_connected(&speaker).await;
    wait_connected(&listener).await;

    // Kick RTP so the room's on_track fires, then give the inbound loop time to start.
    for _ in 0..10 {
        speaker_track
            .write_sample(Bytes::from(vec![0u8; 960]), Duration::from_millis(5))
            .await
            .expect("kick");
        sleep(Duration::from_millis(30)).await;
    }

    let remote = timeout(Duration::from_secs(15), listener_events.tracks.recv())
        .await
        .expect("timed out waiting for the mix track")
        .expect("mix track");
    let received = Arc::new(Mutex::new(Vec::<Detected>::new()));
    let sink = Arc::clone(&received);
    let reader = tokio::spawn(async move {
        while let Ok(sample) = remote.read_sample().await {
            // A multi-frame RTP packet arrives as one sample; classify each 20 ms frame.
            for frame in sample.pcm.chunks(3_840) {
                sink.lock().await.push(detect_frame(frame));
            }
        }
    });

    let schedule = send_schedule(pattern);
    let mut marker = MarkerSource::new();
    let start = Instant::now() + Duration::from_millis(100);
    let group = if matches!(pattern, Pattern::Packet60) {
        3
    } else {
        1
    };
    for first in (0..schedule.len()).step_by(group) {
        sleep_until(start + Duration::from_millis(schedule[first])).await;
        let mut pcm = Vec::new();
        for i in first..first + group {
            pcm.extend_from_slice(&marker.frame(i as u64).pcm);
        }
        speaker_track
            .write_sample(Bytes::from(pcm), FRAME * group as u32)
            .await
            .expect("send");
    }
    // Drain: target depth is at most 160 ms; leave generous slack for the receive side.
    sleep(Duration::from_millis(1_500)).await;

    let stats = room.lock().await.source_stats("speaker").await;
    reader.abort();
    let frames = received.lock().await.clone();
    let report = analyze(&frames);
    if std::env::var("CONTINUITY_TRACE").is_ok() {
        let line: Vec<String> = frames
            .iter()
            .map(|d| match d {
                Detected::Silent => "_".into(),
                Detected::Ambiguous => "?".into(),
                Detected::Index(i) => i.to_string(),
            })
            .collect();
        eprintln!("[continuity {pattern:?} trace] {}", line.join(" "));
    }
    eprintln!("[continuity {pattern:?}] {report:?} stats={stats:?}");

    let _ = speaker.close().await;
    let _ = listener.close().await;
    let _ = room.lock().await.close().await;
    server.destroy_room("continuity").await.ok();
    sleep(Duration::from_millis(500)).await;
    Outcome { report, stats }
}

/// Distinct markers that may go unheard in a clean run: Opus start-up and a few frames it
/// smears into an ambiguous detection.
const CODEC_SLACK: u64 = 6;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paced_input_has_no_gaps_or_duplicates() {
    let o = run_pattern(Pattern::Paced).await;
    let r = &o.report;
    assert_eq!(r.gaps, 0, "{r:?}");
    assert_eq!(r.duplicates, 0, "{r:?}");
    assert_eq!(r.reordered, 0, "{r:?}");
    assert!(
        r.played >= FRAMES - CODEC_SLACK,
        "heard too few markers: {r:?}"
    );
    assert_accounting(&o);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bunched_input_loses_no_frames_beyond_counted_compression() {
    assert_continuity(&run_pattern(Pattern::Bunched).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jittered_input_loses_no_frames_beyond_counted_compression() {
    assert_continuity(&run_pattern(Pattern::Jittered).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "fails until multi-frame Opus packets decode"]
async fn sixty_ms_packets_reach_the_listener_without_loss() {
    assert_continuity(&run_pattern(Pattern::Packet60).await);
}

/// The queue's books balance: every pushed frame is played, absorbed by a counted
/// compression, or still queued; nothing failed to decode.
fn assert_accounting(o: &Outcome) {
    let s = o.stats.expect("source stats");
    assert_eq!(s.decode_errors, 0, "{s:?}");
    assert!(s.pushed >= FRAMES, "{s:?}");
    assert_eq!(
        s.pushed,
        s.played + s.compressed + s.overflow_compressed + s.depth as u64,
        "queue accounting broken: {s:?}"
    );
}

/// Gaps are allowed only as counted compression, duplicates only as counted concealment,
/// ambiguous (merged) frames only where a merge happened, and no marker may vanish.
fn assert_continuity(o: &Outcome) {
    let r = &o.report;
    let s = o.stats.expect("source stats");
    let merged = s.compressed + s.overflow_compressed;
    assert_eq!(r.reordered, 0, "{r:?}");
    assert!(
        r.gaps <= merged,
        "gaps {} exceed counted compressions {s:?}",
        r.gaps
    );
    assert!(
        r.duplicates <= s.concealed,
        "duplicates {} exceed concealed {s:?}",
        r.duplicates
    );
    assert!(
        r.ambiguous <= merged + CODEC_SLACK,
        "unexplained ambiguous frames: {r:?} {s:?}"
    );
    assert!(
        r.played + merged + CODEC_SLACK >= FRAMES,
        "frames vanished: heard {} of {FRAMES}, {s:?}",
        r.played
    );
    assert_accounting(o);
}
