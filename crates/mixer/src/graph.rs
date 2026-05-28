//! Per-listener mix graph with mute matrix and exclude-self routing.

use std::collections::{HashMap, HashSet};

use crate::bus::MixBus;
use crate::frame::{self, Frame, FrameBuffer};

/// Participant or listener identifier.
pub type ParticipantId = String;

/// Conference mix graph: one input buffer per participant, per-listener mute masks.
#[derive(Debug, Default)]
pub struct MixGraph {
    inputs: HashMap<ParticipantId, FrameBuffer>,
    mixing_enabled: bool,
    global_mute: HashSet<ParticipantId>,
    listener_mute: HashMap<(ParticipantId, ParticipantId), bool>,
    /// When set for a listener, only these participants are mixed (allow-list).
    listener_routes: HashMap<ParticipantId, HashSet<ParticipantId>>,
}

impl MixGraph {
    /// Creates an empty mix graph with mixing enabled.
    pub fn new() -> Self {
        Self {
            mixing_enabled: true,
            ..Self::default()
        }
    }

    /// Registers a participant input slot.
    pub fn add_input(&mut self, participant_id: impl Into<ParticipantId>) {
        let id = participant_id.into();
        self.inputs.entry(id).or_insert_with(FrameBuffer::new);
    }

    /// Removes a participant input slot and related mute state.
    pub fn remove_input(&mut self, participant_id: &str) {
        self.inputs.remove(participant_id);
        self.global_mute.remove(participant_id);
        self.listener_mute
            .retain(|(listener, target), _| listener != participant_id && target != participant_id);
        self.listener_routes.retain(|listener, sources| {
            sources.remove(participant_id);
            if sources.is_empty() {
                return false;
            }
            listener != participant_id
        });
    }

    /// When `false`, every [`Self::render_output`] returns silence (room-wide bypass).
    pub fn set_mixing_enabled(&mut self, enabled: bool) {
        self.mixing_enabled = enabled;
    }

    /// Returns whether room-wide mixing is enabled.
    pub fn mixing_enabled(&self) -> bool {
        self.mixing_enabled
    }

    /// Returns true when `target` is muted for all listeners.
    pub fn is_globally_muted(&self, target: &str) -> bool {
        self.global_mute.contains(target)
    }

    /// Returns true when `target` is muted only for `listener`.
    pub fn is_listener_muted(&self, listener: &str, target: &str) -> bool {
        self.listener_mute
            .get(&(listener.to_string(), target.to_string()))
            .copied()
            .unwrap_or(false)
    }

    /// Mutes `target` for all listeners when `muted` is true.
    pub fn set_global_mute(&mut self, target: impl Into<ParticipantId>, muted: bool) {
        let target = target.into();
        if muted {
            self.global_mute.insert(target);
        } else {
            self.global_mute.remove(&target);
        }
    }

    /// Mutes `target` only for `listener` when `muted` is true.
    pub fn set_listener_mute(
        &mut self,
        listener: impl Into<ParticipantId>,
        target: impl Into<ParticipantId>,
        muted: bool,
    ) {
        let key = (listener.into(), target.into());
        if muted {
            self.listener_mute.insert(key, true);
        } else {
            self.listener_mute.remove(&key);
        }
    }

    /// Restricts `listener` to hear only `sources` (allow-list routing matrix).
    ///
    /// When unset, the listener hears all active participants except self (subject to mutes).
    /// Pass an empty slice to clear explicit routing for `listener`.
    pub fn set_listener_sources(
        &mut self,
        listener: impl Into<ParticipantId>,
        sources: &[ParticipantId],
    ) {
        let listener = listener.into();
        if sources.is_empty() {
            self.listener_routes.remove(&listener);
            return;
        }
        self.listener_routes
            .insert(listener, sources.iter().cloned().collect());
    }

    /// Returns explicit allow-list sources for `listener`, if any.
    pub fn listener_sources(&self, listener: &str) -> Option<&HashSet<ParticipantId>> {
        self.listener_routes.get(listener)
    }

    /// Clears explicit routing for `listener` (revert to hear-all-except-self).
    pub fn clear_listener_routes(&mut self, listener: &str) {
        self.listener_routes.remove(listener);
    }

    /// Stores the latest PCM frame for a participant.
    pub fn push_frame(&mut self, participant_id: impl Into<ParticipantId>, frame: Frame) {
        let id = participant_id.into();
        self.inputs
            .entry(id)
            .or_insert_with(FrameBuffer::new)
            .push(frame);
    }

    /// Renders the mixed output for `listener_id`.
    ///
    /// Excludes the listener's own input, applies global and per-listener mute masks,
    /// and returns silence when mixing is disabled.
    pub fn render_output(&self, listener_id: &str) -> Frame {
        if !self.mixing_enabled {
            return frame::silence_frame();
        }

        let mut sources = Vec::new();
        for (participant_id, buffer) in &self.inputs {
            if participant_id == listener_id {
                continue;
            }
            if let Some(allowed) = self.listener_routes.get(listener_id) {
                if !allowed.contains(participant_id) {
                    continue;
                }
            }
            if self.global_mute.contains(participant_id) {
                continue;
            }
            if self
                .listener_mute
                .get(&(listener_id.to_string(), participant_id.clone()))
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            sources.push(buffer.current());
        }

        MixBus::mix(&sources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn tone_frame(participant: u8, amplitude: i16) -> Frame {
        let mut pcm = vec![0u8; frame::FRAME_BYTES];
        let sample = amplitude.wrapping_mul(i16::from(participant));
        for i in 0..frame::SAMPLES_PER_FRAME {
            pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    #[test]
    fn exclude_self_listener_hears_others_not_self() {
        let mut graph = MixGraph::new();
        graph.add_input("alice");
        graph.add_input("bob");
        graph.add_input("carol");

        graph.push_frame("alice", tone_frame(1, 1_000));
        graph.push_frame("bob", tone_frame(2, 1_000));
        graph.push_frame("carol", tone_frame(3, 1_000));

        let out = graph.render_output("bob");
        let sample = i16::from_le_bytes([out.pcm[0], out.pcm[1]]);
        // alice(1000) + carol(3000), bob excluded
        assert_eq!(sample, 4_000);
    }

    #[test]
    fn global_mute_removes_participant_from_all_outputs() {
        let mut graph = MixGraph::new();
        graph.add_input("alice");
        graph.add_input("bob");
        graph.add_input("carol");
        graph.push_frame("alice", tone_frame(1, 5_000));
        graph.push_frame("bob", tone_frame(2, 5_000));
        graph.push_frame("carol", tone_frame(3, 5_000));

        let before = i16::from_le_bytes([
            graph.render_output("bob").pcm[0],
            graph.render_output("bob").pcm[1],
        ]);
        assert_eq!(before, 20_000); // alice + carol (bob excluded)

        graph.set_global_mute("alice", true);

        let bob_out = i16::from_le_bytes([
            graph.render_output("bob").pcm[0],
            graph.render_output("bob").pcm[1],
        ]);
        let carol_out = i16::from_le_bytes([
            graph.render_output("carol").pcm[0],
            graph.render_output("carol").pcm[1],
        ]);

        assert_eq!(bob_out, 15_000); // carol only
        assert_eq!(carol_out, 10_000); // bob only (alice muted for everyone)
    }

    #[test]
    fn listener_mute_only_affects_that_output() {
        let mut graph = MixGraph::new();
        graph.add_input("alice");
        graph.add_input("bob");
        graph.add_input("carol");
        graph.push_frame("alice", tone_frame(1, 2_000));
        graph.push_frame("bob", tone_frame(2, 2_000));
        graph.push_frame("carol", tone_frame(3, 2_000));

        graph.set_listener_mute("bob", "alice", true);

        let bob_out = graph.render_output("bob");
        let carol_out = graph.render_output("carol");

        let bob_sample = i16::from_le_bytes([bob_out.pcm[0], bob_out.pcm[1]]);
        let carol_sample = i16::from_le_bytes([carol_out.pcm[0], carol_out.pcm[1]]);

        // bob: carol only (alice muted for bob, bob excluded)
        assert_eq!(bob_sample, 6_000);
        // carol: alice + bob
        assert_eq!(carol_sample, 6_000);
    }

    #[test]
    fn mixing_disabled_returns_silence_on_all_outputs() {
        let mut graph = MixGraph::new();
        graph.add_input("alice");
        graph.add_input("bob");
        graph.push_frame("alice", tone_frame(1, 10_000));
        graph.push_frame("bob", tone_frame(2, 10_000));

        graph.set_mixing_enabled(false);

        assert_eq!(graph.render_output("alice"), frame::silence_frame());
        assert_eq!(graph.render_output("bob"), frame::silence_frame());
    }

    #[test]
    fn listener_route_allow_list_limits_mix_sources() {
        let mut graph = MixGraph::new();
        graph.add_input("alice");
        graph.add_input("bob");
        graph.add_input("carol");
        graph.push_frame("alice", tone_frame(1, 1_000));
        graph.push_frame("bob", tone_frame(2, 1_000));
        graph.push_frame("carol", tone_frame(3, 1_000));

        graph.set_listener_sources("bob", &["alice".to_string()]);

        let bob_sample = i16::from_le_bytes([
            graph.render_output("bob").pcm[0],
            graph.render_output("bob").pcm[1],
        ]);
        assert_eq!(bob_sample, 1_000);

        graph.clear_listener_routes("bob");
        let bob_all = i16::from_le_bytes([
            graph.render_output("bob").pcm[0],
            graph.render_output("bob").pcm[1],
        ]);
        assert_eq!(bob_all, 4_000);
    }
}
