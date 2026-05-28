//! Mute matrix delegating to [`MixGraph`] mute and mixing state.

use std::sync::Arc;

use node_webrtc_rust_mixer::MixGraph;
use tokio::sync::Mutex;

use crate::error::ConferenceError;

/// Mute scope matching the TypeScript `MuteScope` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteScope {
    Global,
    Listener,
}

/// Wraps shared [`MixGraph`] mute state and mixing gate.
#[derive(Clone)]
pub struct MuteMatrix {
    mix_graph: Arc<Mutex<MixGraph>>,
}

impl MuteMatrix {
    /// Creates a mute matrix bound to the room mix graph.
    pub fn new(mix_graph: Arc<Mutex<MixGraph>>) -> Self {
        Self { mix_graph }
    }

    /// Returns whether room-wide mixing is enabled.
    pub async fn mixing_enabled(&self) -> bool {
        self.mix_graph.lock().await.mixing_enabled()
    }

    /// Enables or disables room-wide mixing (outputs become silence when disabled).
    pub async fn set_mixing_enabled(&self, enabled: bool) {
        let mut graph = self.mix_graph.lock().await;
        graph.set_mixing_enabled(enabled);
    }

    /// Mutes `target` for all listeners or for a single listener.
    pub async fn mute(
        &self,
        target: &str,
        scope: MuteScope,
        listener: Option<&str>,
    ) -> Result<(), ConferenceError> {
        match scope {
            MuteScope::Global => {
                let mut graph = self.mix_graph.lock().await;
                graph.set_global_mute(target, true);
            }
            MuteScope::Listener => {
                let listener_id = listener.ok_or_else(|| {
                    ConferenceError::invalid_mute_scope(
                        "listener scope requires listenerId",
                    )
                })?;
                let mut graph = self.mix_graph.lock().await;
                graph.set_listener_mute(listener_id, target, true);
            }
        }
        Ok(())
    }

    /// Unmutes `target` for all listeners or for a single listener.
    pub async fn unmute(
        &self,
        target: &str,
        scope: MuteScope,
        listener: Option<&str>,
    ) -> Result<(), ConferenceError> {
        match scope {
            MuteScope::Global => {
                let mut graph = self.mix_graph.lock().await;
                graph.set_global_mute(target, false);
            }
            MuteScope::Listener => {
                let listener_id = listener.ok_or_else(|| {
                    ConferenceError::invalid_mute_scope(
                        "listener scope requires listenerId",
                    )
                })?;
                let mut graph = self.mix_graph.lock().await;
                graph.set_listener_mute(listener_id, target, false);
            }
        }
        Ok(())
    }

    /// Returns whether `target` should contribute to `listener`'s mix.
    pub async fn should_include(&self, listener: &str, target: &str) -> bool {
        if listener == target {
            return false;
        }

        let graph = self.mix_graph.lock().await;
        if !graph.mixing_enabled() {
            return false;
        }

        !graph.is_globally_muted(target) && !graph.is_listener_muted(listener, target)
            && graph
                .listener_sources(listener)
                .is_none_or(|allowed| allowed.contains(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use node_webrtc_rust_mixer::{Frame, MixGraph, FRAME_BYTES, SAMPLES_PER_FRAME};

    fn tone_frame(participant: u8, amplitude: i16) -> Frame {
        let mut pcm = vec![0u8; FRAME_BYTES];
        let sample = amplitude.wrapping_mul(i16::from(participant));
        for i in 0..SAMPLES_PER_FRAME {
            pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    #[tokio::test]
    async fn should_include_respects_self_global_and_listener_mutes() {
        let graph = Arc::new(Mutex::new(MixGraph::new()));
        let matrix = MuteMatrix::new(Arc::clone(&graph));

        assert!(!matrix.should_include("bob", "bob").await);
        assert!(matrix.should_include("bob", "alice").await);

        matrix
            .mute("alice", MuteScope::Global, None)
            .await
            .unwrap();
        assert!(!matrix.should_include("bob", "alice").await);
        assert!(!matrix.should_include("carol", "alice").await);

        matrix
            .unmute("alice", MuteScope::Global, None)
            .await
            .unwrap();
        matrix
            .mute("alice", MuteScope::Listener, Some("bob"))
            .await
            .unwrap();
        assert!(!matrix.should_include("bob", "alice").await);
        assert!(matrix.should_include("carol", "alice").await);
    }

    #[tokio::test]
    async fn mixing_disabled_excludes_all_targets() {
        let graph = Arc::new(Mutex::new(MixGraph::new()));
        let matrix = MuteMatrix::new(Arc::clone(&graph));

        matrix.set_mixing_enabled(false).await;
        assert!(!matrix.should_include("bob", "alice").await);
        assert!(!matrix.mixing_enabled().await);

        matrix.set_mixing_enabled(true).await;
        assert!(matrix.mixing_enabled().await);
        assert!(matrix.should_include("bob", "alice").await);
    }

    #[tokio::test]
    async fn listener_scope_requires_listener_id() {
        let graph = Arc::new(Mutex::new(MixGraph::new()));
        let matrix = MuteMatrix::new(graph);

        let err = matrix.mute("alice", MuteScope::Listener, None).await;
        assert!(matches!(
            err,
            Err(ConferenceError::Coded {
                code: crate::error::ConferenceErrorCode::InvalidMuteScope,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn mute_matrix_stays_in_sync_with_mix_graph_render() {
        let graph = Arc::new(Mutex::new(MixGraph::new()));
        graph.lock().await.add_input("alice");
        graph.lock().await.add_input("bob");
        graph
            .lock()
            .await
            .push_frame("alice", tone_frame(1, 5_000));
        graph.lock().await.push_frame("bob", tone_frame(2, 5_000));

        let matrix = MuteMatrix::new(Arc::clone(&graph));
        matrix
            .mute("alice", MuteScope::Global, None)
            .await
            .unwrap();

        let out = graph.lock().await.render_output("bob");
        let sample = i16::from_le_bytes([out.pcm[0], out.pcm[1]]);
        assert_eq!(sample, 0);
    }
}
