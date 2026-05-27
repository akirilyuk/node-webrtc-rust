//! Per-participant peer connection, RTP ingest, and outbound mix rendering.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_core::{
    debug_call, debug_evt, ConnectionState, LocalAudioTrack, PeerConnection, RemoteTrack,
};
use node_webrtc_rust_mixer::{MixGraph, OpusDecoder, FRAME_MS};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time;

use crate::error::ConferenceError;

/// Prime frame: 960 B stereo PCM @ 48 kHz = 5 ms — kicks browser `ontrack` / first RTP.
const PCM_KICK_BYTES: usize = 960;
const PCM_KICK_MS: u64 = 5;

/// Runtime state for one conference participant.
pub struct Participant {
    pub id: String,
    pc: PeerConnection,
    outbound_track: LocalAudioTrack,
    outbound_task: JoinHandle<()>,
}

impl Participant {
    /// Creates a participant, wires track handlers, and spawns ingest/render tasks.
    pub async fn spawn(
        id: String,
        pc: PeerConnection,
        mix_graph: Arc<Mutex<MixGraph>>,
    ) -> Result<Self, ConferenceError> {
        debug_call!(
            "conference::participant",
            "spawn",
            "id={}",
            id
        );

        let outbound_track = LocalAudioTrack::new(
            &format!("{id}-mix-out"),
            &format!("{id}-mix-stream"),
        );
        pc.add_track(outbound_track.as_track_local())
            .await?;

        let participant_id = id.clone();
        let mix_for_handler = Arc::clone(&mix_graph);
        pc.on_track(move |track| {
            debug_evt!(
                "conference::participant",
                "on_track",
                "participant={}, track={}",
                participant_id,
                track.id()
            );
            let mix_graph = Arc::clone(&mix_for_handler);
            let id = participant_id.clone();
            tokio::spawn(run_inbound_loop(id, track, mix_graph));
        });

        {
            let mut graph = mix_graph.lock().await;
            graph.add_input(&id);
        }

        let outbound_task = spawn_outbound_task(
            id.clone(),
            Arc::clone(&mix_graph),
            outbound_track.clone(),
            pc.clone(),
        );

        Ok(Self {
            id,
            pc,
            outbound_track,
            outbound_task,
        })
    }

    /// Returns the underlying peer connection.
    pub fn peer_connection(&self) -> &PeerConnection {
        &self.pc
    }

    /// Returns the personalized outbound mix track.
    pub fn outbound_track(&self) -> &LocalAudioTrack {
        &self.outbound_track
    }

    /// Returns the current peer connection state.
    pub fn connection_state(&self) -> ConnectionState {
        self.pc.connection_state()
    }

    /// Stops tasks, removes the mixer input, and closes the peer connection.
    pub async fn shutdown(&mut self, mix_graph: &Arc<Mutex<MixGraph>>) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::participant",
            "shutdown",
            "id={}",
            self.id
        );

        self.outbound_task.abort();

        {
            let mut graph = mix_graph.lock().await;
            graph.remove_input(&self.id);
        }

        self.pc.close().await?;
        Ok(())
    }
}

async fn run_inbound_loop(
    participant_id: String,
    track: RemoteTrack,
    mix_graph: Arc<Mutex<MixGraph>>,
) {
    debug_evt!(
        "conference::participant",
        "inbound_start",
        "id={}",
        participant_id
    );

    let mut decoder = match OpusDecoder::new() {
        Ok(decoder) => decoder,
        Err(_) => return,
    };

    loop {
        match track.read_rtp().await {
            Ok(packet) => {
                let mixing_enabled = mix_graph.lock().await.mixing_enabled();
                if mixing_enabled {
                    let frame = decoder.decode_payload(&packet.payload);
                    let mut graph = mix_graph.lock().await;
                    graph.push_frame(&participant_id, frame);
                }
            }
            Err(_) => {
                debug_evt!(
                    "conference::participant",
                    "inbound_end",
                    "id={}",
                    participant_id
                );
                break;
            }
        }
    }
}

fn spawn_outbound_task(
    participant_id: String,
    mix_graph: Arc<Mutex<MixGraph>>,
    outbound_track: LocalAudioTrack,
    pc: PeerConnection,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        debug_evt!(
            "conference::participant",
            "outbound_start",
            "id={}",
            participant_id
        );

        let wait_deadline = time::Instant::now() + Duration::from_secs(30);
        loop {
            match pc.connection_state() {
                ConnectionState::Connected => break,
                ConnectionState::Failed | ConnectionState::Closed => return,
                _ => {
                    if time::Instant::now() >= wait_deadline {
                        return;
                    }
                    time::sleep(Duration::from_millis(50)).await;
                }
            }
        }

        let kick = Bytes::from(vec![0u8; PCM_KICK_BYTES]);
        if outbound_track
            .write_sample(kick, Duration::from_millis(PCM_KICK_MS))
            .await
            .is_err()
        {
            return;
        }

        let mut interval = time::interval(Duration::from_millis(FRAME_MS as u64));
        loop {
            interval.tick().await;

            if pc.connection_state() != ConnectionState::Connected {
                break;
            }

            let frame = {
                let graph = mix_graph.lock().await;
                graph.render_output(&participant_id)
            };

            if outbound_track
                .write_sample(frame.pcm, Duration::from_millis(FRAME_MS as u64))
                .await
                .is_err()
            {
                break;
            }
        }

        debug_evt!(
            "conference::participant",
            "outbound_end",
            "id={}",
            participant_id
        );
    })
}
