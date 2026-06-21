//! Integration tests for peer connections.
//!
//! Run serially in CI/local integration (`--test-threads=1`) — parallel runs flake on shared ICE.

use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_core::{
    ConnectionState, DataChannelState, IceServer, LocalAudioTrack, OfferOptions, PeerConnection,
    PeerConnectionConfig, RtpTransceiverDirection, RtpTransceiverInit, TrackKind, TransceiverSource,
};
use tokio::time::{sleep, timeout};

fn test_config() -> PeerConnectionConfig {
    // STUN improves reliability on macOS and under parallel CI load (host candidates alone can flake).
    PeerConnectionConfig {
        ice_servers: vec![IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            ..Default::default()
        }],
        ..Default::default()
    }
}

async fn signal_pair(offer: &PeerConnection, answer: &PeerConnection) {
    let _ = offer
        .create_data_channel("_signal", None)
        .await
        .expect("create signal channel");

    let offer_desc = offer.create_offer(None).await.expect("create offer");
    offer
        .set_local_description(offer_desc)
        .await
        .expect("set local offer");
    offer.gathering_complete().await;

    let local_offer = offer
        .local_description()
        .await
        .expect("local offer missing");
    answer
        .set_remote_description(local_offer)
        .await
        .expect("set remote offer");

    let answer_desc = answer.create_answer(None).await.expect("create answer");
    answer
        .set_local_description(answer_desc)
        .await
        .expect("set local answer");
    answer.gathering_complete().await;

    let local_answer = answer
        .local_description()
        .await
        .expect("local answer missing");
    offer
        .set_remote_description(local_answer)
        .await
        .expect("set remote answer");
}

/// Pause so the next test in the same process can bind ICE ports (serial test harness).
async fn settle_after_peer_activity() {
    sleep(Duration::from_millis(1_000)).await;
}

async fn close_single_peer(pc: &PeerConnection) {
    pc.close().await.expect("close pc");
    pc.close().await.expect("idempotent close pc");
    settle_after_peer_activity().await;
}

async fn close_peer_pair(pc1: &PeerConnection, pc2: &PeerConnection) {
    pc1.close().await.expect("close pc1");
    pc2.close().await.expect("close pc2");
    settle_after_peer_activity().await;
}

async fn wait_for_connection(pc: &PeerConnection) {
    const PER_ATTEMPT: Duration = Duration::from_secs(25);
    for attempt in 1..=2u32 {
        let result = timeout(PER_ATTEMPT, async {
            loop {
                match pc.connection_state() {
                    ConnectionState::Connected => return,
                    ConnectionState::Failed | ConnectionState::Closed => {
                        panic!("connection failed: {:?}", pc.connection_state());
                    }
                    _ => sleep(Duration::from_millis(50)).await,
                }
            }
        })
        .await;
        if result.is_ok() {
            return;
        }
        if attempt < 2 {
            sleep(Duration::from_millis(500)).await;
            continue;
        }
        result.expect("timed out waiting for connection");
    }
}

async fn wait_for_data_channel_open(
    dc: &node_webrtc_rust_core::DataChannel,
) {
    timeout(Duration::from_secs(10), async {
        loop {
            if dc.ready_state() == DataChannelState::Open {
                return;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("timed out waiting for data channel open");
}

#[tokio::test]
async fn test_two_peers_connect() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    signal_pair(&pc1, &pc2).await;

    wait_for_connection(&pc1).await;
    wait_for_connection(&pc2).await;

    close_peer_pair(&pc1, &pc2).await;
}

#[tokio::test]
async fn test_data_channel_round_trip() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    let mut pc2_events = pc2.subscribe_events();

    let dc1 = pc1
        .create_data_channel("chat", None)
        .await
        .expect("create data channel");

    signal_pair(&pc1, &pc2).await;

    let dc2 = timeout(Duration::from_secs(10), pc2_events.data_channels.recv())
        .await
        .expect("timed out waiting for dc2")
        .expect("no data channel received");

    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel(4);
    dc2.on_message(move |msg| {
        let _ = msg_tx.try_send(msg);
    });

    wait_for_data_channel_open(&dc1).await;

    dc1.send_text("hello")
        .await
        .expect("send text");
    dc1.send_binary_slice(&[1, 2, 3])
        .await
        .expect("send binary");

    let text_msg = timeout(Duration::from_secs(10), msg_rx.recv())
        .await
        .expect("timed out waiting for text")
        .expect("no text message");

    assert!(text_msg.is_string);
    assert_eq!(text_msg.data.as_ref(), b"hello");

    let binary_msg = timeout(Duration::from_secs(10), msg_rx.recv())
        .await
        .expect("timed out waiting for binary")
        .expect("no binary message");
    assert!(!binary_msg.is_string);
    assert_eq!(binary_msg.data.as_ref(), &[1, 2, 3]);

    close_peer_pair(&pc1, &pc2).await;
}

#[tokio::test]
async fn test_close_before_data_channel_established() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    let _dc1 = pc1
        .create_data_channel("chat", None)
        .await
        .expect("create data channel");

    signal_pair(&pc1, &pc2).await;

    pc1.close().await.expect("close pc1 before dc open");
    pc2.close().await.expect("close pc2 before dc open");
    settle_after_peer_activity().await;
}

#[tokio::test]
async fn test_add_audio_track() {
    let config = test_config();
    let pc1 = PeerConnection::new(config).await.expect("create pc1");

    let track = LocalAudioTrack::new("audio-1", "stream-1");
    pc1.add_track(track.as_track_local())
        .await
        .expect("add track");

    close_single_peer(&pc1).await;
}

#[tokio::test]
async fn test_audio_track_exchange() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    let mut pc2_events = pc2.subscribe_events();

    let track = LocalAudioTrack::new("audio-1", "stream-1");
    pc1.add_track(track.as_track_local())
        .await
        .expect("add track");

    signal_pair(&pc1, &pc2).await;

    wait_for_connection(&pc1).await;
    wait_for_connection(&pc2).await;

    track
        .write_sample_slice(&[0u8; 960], Duration::from_millis(5))
        .await
        .expect("write sample");

    let remote = timeout(Duration::from_secs(10), pc2_events.tracks.recv())
        .await
        .expect("timed out waiting for remote track")
        .expect("no remote track received");

    assert_eq!(remote.kind(), node_webrtc_rust_core::TrackKind::Audio);
    assert_eq!(remote.id(), "audio-1");

    track
        .write_sample_slice(&[0u8; 3_840], Duration::from_millis(20))
        .await
        .expect("stream pcm");

    let sample = timeout(Duration::from_secs(10), remote.read_sample())
        .await
        .expect("timed out waiting for pcm")
        .expect("read sample");
    assert_eq!(sample.pcm.len(), 3_840);

    close_peer_pair(&pc1, &pc2).await;
}

#[tokio::test]
async fn test_write_sample_with_shared_bytes() {
    let track = LocalAudioTrack::new("audio-1", "stream-1");
    let payload = Bytes::from_static(&[0u8; 3_840]);
    track
        .write_sample(payload, Duration::from_millis(20))
        .await
        .expect("write sample");
}

#[tokio::test]
async fn test_ice_candidate_generation() {
    let config = test_config();
    let pc = PeerConnection::new(config).await.expect("create pc");

    let mut events = pc.subscribe_events();
    let _ = pc
        .create_data_channel("ice-test", None)
        .await
        .expect("create dc");

    let offer = pc.create_offer(None).await.expect("create offer");
    pc.set_local_description(offer)
        .await
        .expect("set local");

    let mut saw_candidate = false;
    let gather_timeout = timeout(Duration::from_secs(10), async {
        while let Some(candidate) = events.ice_candidates.recv().await {
            if let Some(c) = candidate {
                assert!(!c.candidate.is_empty());
                saw_candidate = true;
            } else {
                break;
            }
        }
    })
    .await;

    assert!(gather_timeout.is_ok(), "ICE gathering timed out");
    assert!(saw_candidate, "expected at least one ICE candidate");

    close_single_peer(&pc).await;
}

#[tokio::test]
async fn test_replace_track_swaps_outbound_audio() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    let mut pc2_events = pc2.subscribe_events();

    let track_a = LocalAudioTrack::new("audio-a", "stream-a");
    let sender = pc1
        .add_track(track_a.as_track_local())
        .await
        .expect("add track a");

    signal_pair(&pc1, &pc2).await;
    wait_for_connection(&pc1).await;
    wait_for_connection(&pc2).await;

    track_a
        .write_sample_slice(&[0u8; 960], Duration::from_millis(5))
        .await
        .expect("prime track a");

    let remote_a = timeout(Duration::from_secs(10), pc2_events.tracks.recv())
        .await
        .expect("timed out waiting for remote track")
        .expect("no remote track");
    assert_eq!(remote_a.id(), "audio-a");

    let track_b = LocalAudioTrack::new("audio-b", "stream-b");
    sender
        .replace_track(Some(track_b.as_track_local()))
        .await
        .expect("replace track");

    track_b
        .write_sample_slice(&[0u8; 960], Duration::from_millis(5))
        .await
        .expect("prime track b");

    // Same transceiver — track id on the wire stays the initial id; remote still receives RTP.
    let packet = timeout(Duration::from_secs(10), remote_a.read_rtp())
        .await
        .expect("timed out waiting for rtp after replace")
        .expect("read rtp");
    assert!(!packet.payload.is_empty());

    close_peer_pair(&pc1, &pc2).await;
}

#[tokio::test]
async fn test_offer_to_receive_audio_adds_audio_mline() {
    let pc = PeerConnection::new(test_config())
        .await
        .expect("create pc");
    let desc = pc
        .create_offer(Some(OfferOptions {
            offer_to_receive_audio: true,
            ..Default::default()
        }))
        .await
        .expect("create offer");
    assert!(desc.sdp.contains("m=audio"));
    close_single_peer(&pc).await;
}

#[tokio::test]
async fn test_offer_to_receive_video_returns_error() {
    let pc = PeerConnection::new(test_config())
        .await
        .expect("create pc");
    let err = pc
        .create_offer(Some(OfferOptions {
            offer_to_receive_video: true,
            ..Default::default()
        }))
        .await
        .expect_err("video receive should fail");
    assert!(err.to_string().contains("offerToReceiveVideo"));
    close_single_peer(&pc).await;
}

#[tokio::test]
async fn test_remove_track_detaches_sender() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    let track = LocalAudioTrack::new("audio-1", "stream-1");
    let sender = pc1
        .add_track(track.as_track_local())
        .await
        .expect("add track");

    signal_pair(&pc1, &pc2).await;
    wait_for_connection(&pc1).await;

    pc1.remove_track(&sender).await.expect("remove track");

    close_peer_pair(&pc1, &pc2).await;
}

#[tokio::test]
async fn test_add_transceiver_recvonly_audio() {
    let pc = PeerConnection::new(test_config())
        .await
        .expect("create pc");
    let transceiver = pc
        .add_transceiver(
            TransceiverSource::Kind(TrackKind::Audio),
            Some(RtpTransceiverInit {
                direction: RtpTransceiverDirection::Recvonly,
            }),
        )
        .await
        .expect("add transceiver");

    assert_eq!(transceiver.kind(), TrackKind::Audio);
    assert_eq!(transceiver.direction(), RtpTransceiverDirection::Recvonly);
    assert!(!transceiver.stopped());

    let listed = pc.get_transceivers().await;
    assert_eq!(listed.len(), 1);
    assert_eq!(pc.get_senders().await.len(), 1);
    assert_eq!(pc.get_receivers().await.len(), 1);

    let offer = pc.create_offer(None).await.expect("create offer");
    assert!(offer.sdp.contains("m=audio"));

    close_single_peer(&pc).await;
}

#[tokio::test]
async fn test_add_transceiver_from_local_track() {
    let pc = PeerConnection::new(test_config())
        .await
        .expect("create pc");
    let track = LocalAudioTrack::new("tx-a1", "stream-1");
    let transceiver = pc
        .add_transceiver(
            TransceiverSource::Track(track.as_track_local()),
            None,
        )
        .await
        .expect("add transceiver from track");

    assert_eq!(transceiver.kind(), TrackKind::Audio);
    assert_eq!(transceiver.direction(), RtpTransceiverDirection::Sendrecv);

    close_single_peer(&pc).await;
}

#[tokio::test]
async fn test_transceiver_set_direction_and_stop() {
    let pc = PeerConnection::new(test_config())
        .await
        .expect("create pc");
    let transceiver = pc
        .add_transceiver(
            TransceiverSource::Kind(TrackKind::Audio),
            Some(RtpTransceiverInit {
                direction: RtpTransceiverDirection::Recvonly,
            }),
        )
        .await
        .expect("add transceiver");

    transceiver
        .set_direction(RtpTransceiverDirection::Inactive)
        .await;
    assert_eq!(transceiver.direction(), RtpTransceiverDirection::Inactive);

    transceiver.stop().await.expect("stop transceiver");
    assert!(transceiver.stopped());

    close_single_peer(&pc).await;
}

#[tokio::test]
async fn test_connection_close() {
    let config = test_config();
    let pc1 = PeerConnection::new(config.clone())
        .await
        .expect("create pc1");
    let pc2 = PeerConnection::new(config).await.expect("create pc2");

    signal_pair(&pc1, &pc2).await;
    wait_for_connection(&pc1).await;

    close_peer_pair(&pc1, &pc2).await;

    assert_eq!(pc1.connection_state(), ConnectionState::Closed);
    assert_eq!(pc2.connection_state(), ConnectionState::Closed);
}
