//! Integration tests for room mixing, mute matrix, and lifecycle.

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bytes::Bytes;
use node_webrtc_rust_conference::{
    ConferenceServer, MuteScope, Room, RoomConfig, SignalingMessage, SignalingResponse,
};
use node_webrtc_rust_mixer::{silence_frame, Frame, FRAME_BYTES, SAMPLES_PER_FRAME};
use tokio::sync::Mutex;

/// Injected clock origin: sources push at `t0()`, listeners render at `ready()`.
fn t0() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

/// Two media ticks after `t0()`: a lone frame has waited out its prefill and plays.
fn ready() -> Instant {
    t0() + Duration::from_millis(40)
}

fn tone_frame(participant: u8, amplitude: i16) -> Frame {
    let mut pcm = vec![0u8; FRAME_BYTES];
    let sample = amplitude.wrapping_mul(i16::from(participant));
    for i in 0..SAMPLES_PER_FRAME {
        pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
    }
    Frame::new(Bytes::from(pcm), None)
}

async fn setup_room(ids: &[&str]) -> Arc<Mutex<Room>> {
    let server = ConferenceServer::new();
    let room_handle = server
        .create_room("test-room", RoomConfig::default())
        .await
        .expect("create room");

    {
        let mut room = room_handle.lock().await;
        for id in ids {
            room.add_participant(id).await.expect("add participant");
        }
    }

    room_handle
}

#[tokio::test]
async fn listener_mix_excludes_self() {
    let room_handle = setup_room(&["alice", "bob"]).await;
    let room = room_handle.lock().await;

    room.inject_frame_at("alice", tone_frame(1, 3_000), t0())
        .await
        .unwrap();
    room.inject_frame_at("bob", tone_frame(2, 3_000), t0())
        .await
        .unwrap();

    let bob_out = room.render_output_at("bob", ready()).await.unwrap();
    let bob_sample = i16::from_le_bytes([bob_out.pcm[0], bob_out.pcm[1]]);
    assert_eq!(bob_sample, 3_000);

    let alice_out = room.render_output_at("alice", ready()).await.unwrap();
    let alice_sample = i16::from_le_bytes([alice_out.pcm[0], alice_out.pcm[1]]);
    assert_eq!(alice_sample, 6_000);
}

#[tokio::test]
async fn global_mute_excludes_participant_from_all_mixes() {
    let room_handle = setup_room(&["alice", "bob", "carol"]).await;
    let room = room_handle.lock().await;

    room.inject_frame_at("alice", tone_frame(1, 2_000), t0())
        .await
        .unwrap();
    room.inject_frame_at("bob", tone_frame(2, 2_000), t0())
        .await
        .unwrap();
    room.inject_frame_at("carol", tone_frame(3, 2_000), t0())
        .await
        .unwrap();

    room.mute_participant("alice", MuteScope::Global, None)
        .await
        .unwrap();

    let bob_out = room.render_output_at("bob", ready()).await.unwrap();
    let bob_sample = i16::from_le_bytes([bob_out.pcm[0], bob_out.pcm[1]]);
    assert_eq!(bob_sample, 6_000);

    let carol_out = room.render_output_at("carol", ready()).await.unwrap();
    let carol_sample = i16::from_le_bytes([carol_out.pcm[0], carol_out.pcm[1]]);
    assert_eq!(carol_sample, 4_000);
}

#[tokio::test]
async fn mixing_disabled_returns_silence() {
    let room_handle = setup_room(&["alice", "bob"]).await;
    let room = room_handle.lock().await;

    room.inject_frame_at("alice", tone_frame(1, 5_000), t0())
        .await
        .unwrap();
    room.inject_frame_at("bob", tone_frame(2, 5_000), t0())
        .await
        .unwrap();

    room.set_mixing_enabled(false).await;
    assert!(!room.mixing_enabled().await);

    let out = room.render_output_at("bob", ready()).await.unwrap();
    assert_eq!(out, silence_frame());
}

#[tokio::test]
async fn kick_removes_participant_from_mix() {
    let server = ConferenceServer::new();
    let room_handle = server
        .create_room("test-room", RoomConfig::default())
        .await
        .expect("create room");

    {
        let mut room = room_handle.lock().await;
        room.add_participant("alice").await.unwrap();
        room.add_participant("bob").await.unwrap();

        room.inject_frame_at("alice", tone_frame(1, 4_000), t0())
            .await
            .unwrap();
        room.inject_frame_at("bob", tone_frame(2, 4_000), t0())
            .await
            .unwrap();

        room.kick_participant("alice", Some("test")).await.unwrap();

        assert_eq!(room.list_participants().len(), 1);
        assert!(room.render_output_at("alice", ready()).await.is_err());

        let bob_out = room.render_output_at("bob", ready()).await.unwrap();
        let bob_sample = i16::from_le_bytes([bob_out.pcm[0], bob_out.pcm[1]]);
        assert_eq!(bob_sample, 0);
    }

    server.destroy_room("test-room").await.unwrap();
}

#[tokio::test]
async fn join_signaling_creates_server_offer() {
    let server = ConferenceServer::new();
    let room_handle = server
        .create_room("sig-room", RoomConfig::default())
        .await
        .unwrap();

    let responses = {
        let mut room = room_handle.lock().await;
        room.handle_signaling(SignalingMessage::Join {
            participant_id: "alice".into(),
            room_id: Some("sig-room".into()),
        })
        .await
        .unwrap()
    };

    assert_eq!(responses.len(), 1);
    match &responses[0] {
        SignalingResponse::Offer {
            participant_id,
            sdp,
        } => {
            assert_eq!(participant_id, "alice");
            assert!(sdp.contains("v=0"));
        }
        _ => panic!("expected offer response"),
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let room = room_handle.lock().await;
    assert_eq!(room.list_participants().len(), 1);
}

#[tokio::test]
async fn bunched_injected_frames_all_reach_the_listener_in_order() {
    let room_handle = setup_room(&["alice", "bob"]).await;
    let room = room_handle.lock().await;

    // 10 frames at one instant (value = 1000 * (i + 1)); bob must hear all 10, one per tick.
    for i in 0..10u8 {
        room.inject_frame_at("alice", tone_frame(1, 1_000 * i16::from(i + 1)), t0())
            .await
            .unwrap();
    }
    let mut heard = Vec::new();
    for tick in 1..=10u64 {
        let out = room
            .render_output_at("bob", t0() + Duration::from_millis(20 * tick))
            .await
            .unwrap();
        heard.push(i16::from_le_bytes([out.pcm[0], out.pcm[1]]));
    }
    let expected: Vec<i16> = (1..=10).map(|i| 1_000 * i).collect();
    assert_eq!(heard, expected);

    let stats = room.source_stats("alice").await.unwrap();
    assert_eq!(stats.pushed, 10);
    assert_eq!(stats.played, 10);
    assert_eq!(stats.compressed, 0);
}
