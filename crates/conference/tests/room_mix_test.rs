//! Integration tests for room mixing, mute matrix, and lifecycle.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_conference::{
    ConferenceServer, MuteScope, Room, RoomConfig, SignalingMessage, SignalingResponse,
};
use node_webrtc_rust_mixer::{silence_frame, Frame, FRAME_BYTES, SAMPLES_PER_FRAME};
use tokio::sync::Mutex;

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

    room.inject_frame("alice", tone_frame(1, 3_000))
        .await
        .unwrap();
    room.inject_frame("bob", tone_frame(2, 3_000))
        .await
        .unwrap();

    let bob_out = room.render_output("bob").await.unwrap();
    let bob_sample = i16::from_le_bytes([bob_out.pcm[0], bob_out.pcm[1]]);
    assert_eq!(bob_sample, 3_000);

    let alice_out = room.render_output("alice").await.unwrap();
    let alice_sample = i16::from_le_bytes([alice_out.pcm[0], alice_out.pcm[1]]);
    assert_eq!(alice_sample, 6_000);
}

#[tokio::test]
async fn global_mute_excludes_participant_from_all_mixes() {
    let room_handle = setup_room(&["alice", "bob", "carol"]).await;
    let room = room_handle.lock().await;

    room.inject_frame("alice", tone_frame(1, 2_000))
        .await
        .unwrap();
    room.inject_frame("bob", tone_frame(2, 2_000))
        .await
        .unwrap();
    room.inject_frame("carol", tone_frame(3, 2_000))
        .await
        .unwrap();

    room.mute_participant("alice", MuteScope::Global, None)
        .await
        .unwrap();

    let bob_out = room.render_output("bob").await.unwrap();
    let bob_sample = i16::from_le_bytes([bob_out.pcm[0], bob_out.pcm[1]]);
    assert_eq!(bob_sample, 6_000);

    let carol_out = room.render_output("carol").await.unwrap();
    let carol_sample = i16::from_le_bytes([carol_out.pcm[0], carol_out.pcm[1]]);
    assert_eq!(carol_sample, 4_000);
}

#[tokio::test]
async fn mixing_disabled_returns_silence() {
    let room_handle = setup_room(&["alice", "bob"]).await;
    let room = room_handle.lock().await;

    room.inject_frame("alice", tone_frame(1, 5_000))
        .await
        .unwrap();
    room.inject_frame("bob", tone_frame(2, 5_000))
        .await
        .unwrap();

    room.set_mixing_enabled(false).await;
    assert!(!room.mixing_enabled().await);

    let out = room.render_output("bob").await.unwrap();
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

        room.inject_frame("alice", tone_frame(1, 4_000))
            .await
            .unwrap();
        room.inject_frame("bob", tone_frame(2, 4_000))
            .await
            .unwrap();

        room.kick_participant("alice", Some("test"))
            .await
            .unwrap();

        assert_eq!(room.list_participants().len(), 1);
        assert!(room.render_output("alice").await.is_err());

        let bob_out = room.render_output("bob").await.unwrap();
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
        SignalingResponse::Offer { participant_id, sdp } => {
            assert_eq!(participant_id, "alice");
            assert!(sdp.contains("v=0"));
        }
        _ => panic!("expected offer response"),
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    let room = room_handle.lock().await;
    assert_eq!(room.list_participants().len(), 1);
}
