//! Per-participant peer connection, RTP ingest, and outbound mix rendering.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use node_webrtc_rust_core::{
    debug_call, debug_evt, ConnectionState, LocalAudioTrack, PeerConnection, RemoteTrack,
};
use node_webrtc_rust_denoise::Stereo48kRnnoise;
use node_webrtc_rust_mixer::{Frame, MixGraph, OpusDecoder, FRAME_MS};
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
        noise_suppression: bool,
    ) -> Result<Self, ConferenceError> {
        debug_call!("conference::participant", "spawn", "id={}", id);

        let outbound_track =
            LocalAudioTrack::new(&format!("{id}-mix-out"), &format!("{id}-mix-stream"));
        let _ = pc.add_track(outbound_track.as_track_local()).await?;

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
            tokio::spawn(run_inbound_loop(id, track, mix_graph, noise_suppression));
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
    pub async fn shutdown(
        &mut self,
        mix_graph: &Arc<Mutex<MixGraph>>,
    ) -> Result<(), ConferenceError> {
        debug_call!("conference::participant", "shutdown", "id={}", self.id);

        self.outbound_task.abort();

        {
            let mut graph = mix_graph.lock().await;
            graph.remove_input(&self.id);
        }

        self.pc.close().await?;
        Ok(())
    }
}

/// Applies optional RNNoise to a mixer frame (testable without RTP).
pub fn apply_noise_suppression(denoiser: Option<&mut Stereo48kRnnoise>, frame: Frame) -> Frame {
    match denoiser {
        None => frame,
        Some(denoise) => {
            let pcm = Bytes::from(denoise.process_s16le_stereo(frame.pcm.as_ref()));
            Frame::new(pcm, frame.timestamp_us)
        }
    }
}

async fn run_inbound_loop(
    participant_id: String,
    track: RemoteTrack,
    mix_graph: Arc<Mutex<MixGraph>>,
    noise_suppression: bool,
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

    let mut denoiser = if noise_suppression {
        Some(Stereo48kRnnoise::new())
    } else {
        None
    };

    loop {
        match track.read_rtp().await {
            Ok(packet) => {
                let mixing_enabled = mix_graph.lock().await.mixing_enabled();
                if mixing_enabled {
                    let frame = decoder.decode_payload(&packet.payload);
                    let frame = apply_noise_suppression(denoiser.as_mut(), frame);
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

#[cfg(test)]
mod noise_suppression_tests {
    use super::*;
    use node_webrtc_rust_denoise::{stereo_pcm_rms, Stereo48kRnnoise};
    use node_webrtc_rust_mixer::{FRAME_BYTES, SAMPLES_PER_FRAME};

    fn white_noise_frame(seed: u32) -> Frame {
        let mut state = seed.max(1);
        let mut pcm = vec![0u8; FRAME_BYTES];
        for i in 0..SAMPLES_PER_FRAME {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let sample = ((state >> 16) as i16).wrapping_mul(4);
            pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    #[test]
    fn disabled_noise_suppression_is_identity() {
        let frame = white_noise_frame(1);
        let out = apply_noise_suppression(None, frame.clone());
        assert_eq!(out.pcm, frame.pcm);
    }

    #[test]
    fn enabled_noise_suppression_reduces_white_noise_rms_after_warmup() {
        let mut denoiser = Stereo48kRnnoise::new();
        for i in 0..5 {
            let warm = white_noise_frame(i);
            let _ = apply_noise_suppression(Some(&mut denoiser), warm);
        }
        let noisy = white_noise_frame(99);
        let input_rms = stereo_pcm_rms(noisy.pcm.as_ref());
        let out = apply_noise_suppression(Some(&mut denoiser), noisy);
        let output_rms = stereo_pcm_rms(out.pcm.as_ref());
        assert!(input_rms > 0.05);
        assert!(output_rms < input_rms);
    }
}
