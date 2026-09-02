//! Per-listener mix graph with mute matrix, exclude-self routing, and positional pan.

use std::collections::{HashMap, HashSet};

use crate::bus::MixBus;
use crate::frame::{self, Frame, FrameBuffer};
use crate::spatial::{
    apply_pan_gains, pan_frame_with_placement, pan_gains_placement, pan_gains_positional,
    ClientPose, DistanceParams, MixPlacement,
};

/// Participant or listener identifier.
pub type ParticipantId = String;

/// Conference mix graph: one input buffer per participant, per-listener mute masks,
/// optional poses, and exclusive mix groups.
#[derive(Debug)]
pub struct MixGraph {
    inputs: HashMap<ParticipantId, FrameBuffer>,
    mixing_enabled: bool,
    global_mute: HashSet<ParticipantId>,
    listener_mute: HashMap<(ParticipantId, ParticipantId), bool>,
    /// When set for a listener, only these participants are mixed (allow-list).
    listener_routes: HashMap<ParticipantId, HashSet<ParticipantId>>,
    poses: HashMap<ParticipantId, ClientPose>,
    positional_enabled: bool,
    default_mix_placement: MixPlacement,
    tts_mix_placement: MixPlacement,
    tts_poses: HashMap<ParticipantId, ClientPose>,
    distance_params: DistanceParams,
    /// group_id → members
    groups: HashMap<String, HashSet<ParticipantId>>,
    /// participant → group_id
    member_group: HashMap<ParticipantId, String>,
}

impl Default for MixGraph {
    fn default() -> Self {
        Self {
            inputs: HashMap::new(),
            mixing_enabled: true,
            global_mute: HashSet::new(),
            listener_mute: HashMap::new(),
            listener_routes: HashMap::new(),
            poses: HashMap::new(),
            positional_enabled: false,
            default_mix_placement: MixPlacement::Center,
            tts_mix_placement: MixPlacement::Center,
            tts_poses: HashMap::new(),
            distance_params: DistanceParams::default(),
            groups: HashMap::new(),
            member_group: HashMap::new(),
        }
    }
}

impl MixGraph {
    /// Creates an empty mix graph with mixing enabled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a participant input slot.
    pub fn add_input(&mut self, participant_id: impl Into<ParticipantId>) {
        let id = participant_id.into();
        self.inputs.entry(id).or_insert_with(FrameBuffer::new);
    }

    /// Removes a participant input slot and related mute / pose / group state.
    pub fn remove_input(&mut self, participant_id: &str) {
        self.inputs.remove(participant_id);
        self.global_mute.remove(participant_id);
        self.poses.remove(participant_id);
        self.tts_poses.remove(participant_id);
        self.listener_mute
            .retain(|(listener, target), _| listener != participant_id && target != participant_id);
        self.listener_routes.retain(|listener, sources| {
            sources.remove(participant_id);
            if sources.is_empty() {
                return false;
            }
            listener != participant_id
        });

        if let Some(group_id) = self.member_group.remove(participant_id) {
            if let Some(members) = self.groups.get_mut(&group_id) {
                members.remove(participant_id);
            }
            self.rebuild_group_routes(&group_id);
        }
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

    /// Stores the latest known pose for a participant.
    pub fn set_pose(&mut self, participant_id: impl Into<ParticipantId>, pose: ClientPose) {
        self.poses.insert(participant_id.into(), pose);
    }

    /// Removes a stored pose for a participant.
    pub fn clear_pose(&mut self, participant_id: &str) {
        self.poses.remove(participant_id);
    }

    /// Returns the last known pose for a participant, if any.
    pub fn pose(&self, participant_id: &str) -> Option<ClientPose> {
        self.poses.get(participant_id).copied()
    }

    /// When `true`, [`Self::render_output`] pans sources from last known poses.
    /// When `false`, sources use [`Self::default_mix_placement`].
    pub fn set_positional_enabled(&mut self, enabled: bool) {
        self.positional_enabled = enabled;
    }

    /// Returns whether positional panning is active.
    pub fn positional_enabled(&self) -> bool {
        self.positional_enabled
    }

    /// Default named placement for client sources when positional mixing is off.
    pub fn set_default_mix_placement(&mut self, placement: MixPlacement) {
        self.default_mix_placement = placement;
    }

    /// Returns the default client placement when positional mixing is off.
    pub fn default_mix_placement(&self) -> MixPlacement {
        self.default_mix_placement
    }

    /// Named placement applied to TTS frames via [`Self::pan_tts_frame`].
    pub fn set_tts_mix_placement(&mut self, placement: MixPlacement) {
        self.tts_mix_placement = placement;
    }

    /// Returns the TTS mix placement.
    pub fn tts_mix_placement(&self) -> MixPlacement {
        self.tts_mix_placement
    }

    /// Sets a world-space TTS speaker pose for `participant_id` (used when positional mixing is on).
    pub fn set_tts_pose(&mut self, participant_id: impl Into<ParticipantId>, pose: ClientPose) {
        self.tts_poses.insert(participant_id.into(), pose);
    }

    /// Clears the live TTS pose for `participant_id`; named [`Self::tts_mix_placement`] applies again.
    pub fn clear_tts_pose(&mut self, participant_id: &str) {
        self.tts_poses.remove(participant_id);
    }

    /// Returns the current TTS speaker pose for `participant_id`, if any.
    pub fn tts_pose(&self, participant_id: &str) -> Option<ClientPose> {
        self.tts_poses.get(participant_id).copied()
    }

    /// Sets distance attenuation parameters for positional panning.
    ///
    /// Defaults: reference distance 1, max distance 50, rolloff 1.
    pub fn set_distance_params(&mut self, params: DistanceParams) {
        self.distance_params = params;
    }

    /// Returns distance attenuation parameters.
    pub fn distance_params(&self) -> DistanceParams {
        self.distance_params
    }

    /// Defines or replaces a mix group's membership and rebuilds listener routes.
    pub fn set_group_members(&mut self, group_id: impl Into<String>, members: &[ParticipantId]) {
        let group_id = group_id.into();
        let member_set: HashSet<ParticipantId> = members.iter().cloned().collect();

        for id in &member_set {
            if let Some(old) = self.member_group.insert(id.clone(), group_id.clone()) {
                if old != group_id {
                    if let Some(old_members) = self.groups.get_mut(&old) {
                        old_members.remove(id);
                    }
                    self.rebuild_group_routes(&old);
                }
            }
        }

        self.groups.insert(group_id.clone(), member_set);
        self.rebuild_group_routes(&group_id);
    }

    /// Moves `participant_id` exclusively into `group_id`, updating all affected routes.
    ///
    /// The participant is removed from any previous group. Listener routes for both
    /// the old and new group are rebuilt on the next [`Self::render_output`].
    pub fn move_to_group(
        &mut self,
        participant_id: impl Into<ParticipantId>,
        group_id: impl Into<String>,
    ) {
        let participant_id = participant_id.into();
        let group_id = group_id.into();

        let old_group = self.member_group.remove(&participant_id);
        if let Some(ref old) = old_group {
            if let Some(members) = self.groups.get_mut(old) {
                members.remove(&participant_id);
            }
        }

        self.groups
            .entry(group_id.clone())
            .or_default()
            .insert(participant_id.clone());
        self.member_group.insert(participant_id, group_id.clone());

        if let Some(old) = old_group {
            if old != group_id {
                self.rebuild_group_routes(&old);
            }
        }
        self.rebuild_group_routes(&group_id);
    }

    /// Removes `participant_id` from its mix group (ungrouped: hears nobody).
    pub fn remove_from_group(&mut self, participant_id: &str) {
        let Some(group_id) = self.member_group.remove(participant_id) else {
            return;
        };
        if let Some(members) = self.groups.get_mut(&group_id) {
            members.remove(participant_id);
        }
        self.listener_routes.remove(participant_id);
        self.rebuild_group_routes(&group_id);
    }

    fn rebuild_group_routes(&mut self, group_id: &str) {
        let Some(members) = self.groups.get(group_id).cloned() else {
            return;
        };

        for listener in &members {
            let sources: Vec<ParticipantId> =
                members.iter().filter(|m| *m != listener).cloned().collect();
            if sources.is_empty() {
                self.listener_routes.remove(listener);
            } else {
                self.set_listener_sources(listener.clone(), &sources);
            }
        }

        // Participants removed from the group but not in any group hear nobody.
        self.listener_routes
            .retain(|listener, _| self.member_group.contains_key(listener));
    }

    /// Stores the latest PCM frame for a participant.
    pub fn push_frame(&mut self, participant_id: impl Into<ParticipantId>, frame: Frame) {
        let id = participant_id.into();
        self.inputs
            .entry(id)
            .or_insert_with(FrameBuffer::new)
            .push(frame);
    }

    /// Pans a TTS frame for `listener_id`.
    ///
    /// When positional mixing is on and a TTS pose is set for `listener_id`, pans against
    /// that listener's pose; otherwise uses [`Self::tts_mix_placement`].
    pub fn pan_tts_frame(&self, frame: &Frame, listener_id: &str) -> Frame {
        if self.positional_enabled {
            if let Some(tts_pose) = self.tts_poses.get(listener_id).copied() {
                let listener_pose = self
                    .poses
                    .get(listener_id)
                    .copied()
                    .unwrap_or_else(ClientPose::center);
                let gains = pan_gains_positional(listener_pose, tts_pose, self.distance_params);
                return apply_pan_gains(frame, gains);
            }
        }
        pan_frame_with_placement(frame, self.tts_mix_placement, self.distance_params)
    }

    fn pan_source_for_listener(&self, listener_id: &str, source_id: &str, frame: &Frame) -> Frame {
        let gains = if self.positional_enabled {
            let listener_pose = self
                .poses
                .get(listener_id)
                .copied()
                .unwrap_or_else(ClientPose::center);
            let source_pose = self
                .poses
                .get(source_id)
                .copied()
                .unwrap_or_else(ClientPose::center);
            pan_gains_positional(listener_pose, source_pose, self.distance_params)
        } else {
            pan_gains_placement(self.default_mix_placement, self.distance_params)
        };
        apply_pan_gains(frame, gains)
    }

    /// Renders the mixed output for `listener_id`.
    ///
    /// Excludes the listener's own input, applies global and per-listener mute masks,
    /// pans each source (positional or named placement), and returns silence when mixing
    /// is disabled.
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
            let raw = buffer.current();
            sources.push(self.pan_source_for_listener(listener_id, participant_id, &raw));
        }

        MixBus::mix(&sources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{Quat, Vec3};
    use bytes::Bytes;

    fn tone_frame(participant: u8, amplitude: i16) -> Frame {
        let mut pcm = vec![0u8; frame::FRAME_BYTES];
        let sample = amplitude.wrapping_mul(i16::from(participant));
        for i in 0..frame::SAMPLES_PER_FRAME {
            pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    fn mono_stereo(amplitude: i16) -> Frame {
        let mut pcm = vec![0u8; frame::FRAME_BYTES];
        for i in 0..frame::SAMPLES_PER_FRAME / 2 {
            let base = i * 4;
            pcm[base..base + 2].copy_from_slice(&amplitude.to_le_bytes());
            pcm[base + 2..base + 4].copy_from_slice(&amplitude.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    fn first_lr(frame: &Frame) -> (i16, i16) {
        (
            i16::from_le_bytes([frame.pcm[0], frame.pcm[1]]),
            i16::from_le_bytes([frame.pcm[2], frame.pcm[3]]),
        )
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
        assert_eq!(before, 20_000);

        graph.set_global_mute("alice", true);

        let bob_out = i16::from_le_bytes([
            graph.render_output("bob").pcm[0],
            graph.render_output("bob").pcm[1],
        ]);
        let carol_out = i16::from_le_bytes([
            graph.render_output("carol").pcm[0],
            graph.render_output("carol").pcm[1],
        ]);

        assert_eq!(bob_out, 15_000);
        assert_eq!(carol_out, 10_000);
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

        assert_eq!(bob_sample, 6_000);
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

    #[test]
    fn groups_isolate_abc_from_df() {
        let mut graph = MixGraph::new();
        for id in ["A", "B", "C", "D", "F"] {
            graph.add_input(id);
            graph.push_frame(id, mono_stereo(5_000));
        }

        graph.set_group_members("g1", &["A".into(), "B".into(), "C".into()]);
        graph.set_group_members("g2", &["D".into(), "F".into()]);

        let a_out = graph.render_output("A");
        let d_out = graph.render_output("D");
        let (a_l, _) = first_lr(&a_out);
        let (d_l, _) = first_lr(&d_out);

        // A hears B + C (two sources), D hears F only (one source)
        assert!(a_l > d_l);
        assert!(d_l > 0);
    }

    #[test]
    fn move_to_group_mid_graph_updates_routes() {
        let mut graph = MixGraph::new();
        for id in ["A", "B", "C", "D", "F"] {
            graph.add_input(id);
            graph.push_frame(id, mono_stereo(8_000));
        }

        graph.set_group_members("g1", &["A".into(), "B".into(), "C".into()]);
        graph.set_group_members("g2", &["D".into(), "F".into()]);

        // A hears B and C before move
        let before = graph.render_output("A");
        let (before_l, _) = first_lr(&before);
        assert!(before_l > 0);

        graph.move_to_group("A", "g2");

        // A no longer hears B (not in g2 allow-list)
        graph.push_frame("B", mono_stereo(20_000));
        let a_after = graph.render_output("A");
        let (a_l, _) = first_lr(&a_after);
        // A hears D and F only
        assert!(a_l > 0);

        // D now hears A
        let d_out = graph.render_output("D");
        let (d_l, _) = first_lr(&d_out);
        assert!(d_l > 0);

        // B no longer hears A — only C remains in g1 for B
        let b_out = graph.render_output("B");
        let (b_l, _) = first_lr(&b_out);
        assert!(b_l > 0);
        assert!(b_l < before_l);
    }

    #[test]
    fn positional_on_source_right_is_louder_in_right() {
        let mut graph = MixGraph::new();
        graph.add_input("listener");
        graph.add_input("source");
        graph.set_positional_enabled(true);

        graph.set_pose(
            "listener",
            ClientPose {
                position: Vec3::ZERO,
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_pose(
            "source",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );

        graph.push_frame("source", mono_stereo(10_000));
        graph.set_listener_sources("listener", &["source".to_string()]);

        let out = graph.render_output("listener");
        let (l, r) = first_lr(&out);
        assert!(r > l);
    }

    #[test]
    fn positional_off_uses_named_placement_not_last_pose() {
        let mut graph = MixGraph::new();
        graph.add_input("listener");
        graph.add_input("source");

        graph.set_pose(
            "source",
            ClientPose {
                position: Vec3::try_new(5.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );

        graph.set_positional_enabled(true);
        graph.push_frame("source", mono_stereo(10_000));
        graph.set_listener_sources("listener", &["source".to_string()]);
        let on = graph.render_output("listener");
        let (_, r_on) = first_lr(&on);

        graph.set_positional_enabled(false);
        graph.set_default_mix_placement(MixPlacement::Left);
        let off_left = graph.render_output("listener");
        let (l_off, r_off) = first_lr(&off_left);
        assert!(l_off > r_off);

        graph.set_positional_enabled(true);
        let on_again = graph.render_output("listener");
        let (l_again, r_again) = first_lr(&on_again);
        assert!(r_again > l_again);
        assert!((r_again - r_on).abs() < 500);
    }

    #[test]
    fn tts_mix_placement_right_pans_frame() {
        let graph = MixGraph {
            tts_mix_placement: MixPlacement::Right,
            ..MixGraph::new()
        };
        let frame = mono_stereo(10_000);
        let panned = graph.pan_tts_frame(&frame, "listener");
        let (l, r) = first_lr(&panned);
        assert!(r > l);
    }

    #[test]
    fn tts_pose_right_pans_frame_when_positional_on() {
        let mut graph = MixGraph::new();
        graph.set_positional_enabled(true);
        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        let frame = mono_stereo(10_000);
        let panned = graph.pan_tts_frame(&frame, "listener");
        let (l, r) = first_lr(&panned);
        assert!(r > l);
    }

    #[test]
    fn tts_pose_ignored_when_positional_off() {
        let mut graph = MixGraph::new();
        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_tts_mix_placement(MixPlacement::Center);
        let frame = mono_stereo(10_000);
        let panned = graph.pan_tts_frame(&frame, "listener");
        let (l, r) = first_lr(&panned);
        assert!((l - r).abs() < 500);
    }

    #[test]
    fn tts_pose_clear_falls_back_to_named_placement() {
        let mut graph = MixGraph::new();
        graph.set_positional_enabled(true);
        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_tts_mix_placement(MixPlacement::Right);
        graph.clear_tts_pose("listener");
        let frame = mono_stereo(10_000);
        let panned = graph.pan_tts_frame(&frame, "listener");
        let (l, r) = first_lr(&panned);
        assert!(r > l);
    }

    #[test]
    fn tts_pose_per_listener_independent() {
        let mut graph = MixGraph::new();
        graph.set_positional_enabled(true);
        graph.set_tts_pose(
            "a",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_tts_pose(
            "b",
            ClientPose {
                position: Vec3::try_new(-3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        let frame = mono_stereo(10_000);
        let (a_l, a_r) = first_lr(&graph.pan_tts_frame(&frame, "a"));
        let (b_l, b_r) = first_lr(&graph.pan_tts_frame(&frame, "b"));
        assert!(a_r > a_l);
        assert!(b_l > b_r);
    }

    fn sine_stereo_frame(freq_hz: f32, amplitude: i16, phase: &mut f64) -> Frame {
        let sample_rate = f64::from(frame::SAMPLE_RATE);
        let mut pcm = vec![0u8; frame::FRAME_BYTES];
        for i in 0..frame::SAMPLES_PER_CHANNEL {
            let t = *phase / sample_rate;
            let sample = (f64::from(amplitude)
                * (2.0 * std::f64::consts::PI * f64::from(freq_hz) * t).sin())
                as i16;
            *phase += 1.0;
            let base = i * 4;
            pcm[base..base + 2].copy_from_slice(&sample.to_le_bytes());
            pcm[base + 2..base + 4].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    fn extract_channel_i16(pcm: &[u8], channel: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(frame::SAMPLES_PER_CHANNEL);
        let mut i = channel * 2;
        while i + 1 < pcm.len() {
            out.push(i16::from_le_bytes([pcm[i], pcm[i + 1]]));
            i += 4;
        }
        out
    }

    fn goertzel_power(samples: &[i16], sample_rate: u32, target_freq: f32) -> f64 {
        let n = samples.len();
        if n == 0 {
            return 0.0;
        }
        let k = (0.5 + (n as f64) * f64::from(target_freq) / f64::from(sample_rate)).floor() as usize;
        let omega = (2.0 * std::f64::consts::PI * k as f64) / n as f64;
        let coeff = 2.0 * omega.cos();
        let mut s0 = 0.0f64;
        let mut s1 = 0.0f64;
        let mut s2 = 0.0f64;
        for &sample in samples {
            let x = f64::from(sample);
            s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coeff * s1 * s2
    }

    fn setup_three_client_positional_graph(c1_x: f32, c3_x: f32) -> MixGraph {
        let mut graph = MixGraph::new();
        for id in ["c1", "c2", "c3"] {
            graph.add_input(id);
        }
        graph.set_positional_enabled(true);
        graph.set_pose(
            "c2",
            ClientPose {
                position: Vec3::ZERO,
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_pose(
            "c1",
            ClientPose {
                position: Vec3::try_new(c1_x, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_pose(
            "c3",
            ClientPose {
                position: Vec3::try_new(c3_x, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        graph.set_listener_sources("c2", &["c1".to_string(), "c3".to_string()]);
        graph
    }

    fn render_listener_accumulated(
        graph: &mut MixGraph,
        frame_count: usize,
    ) -> (Vec<i16>, Vec<i16>) {
        let mut phase_c1 = 0.0;
        let mut phase_c3 = 0.0;
        let mut left = Vec::with_capacity(frame_count * frame::SAMPLES_PER_CHANNEL);
        let mut right = Vec::with_capacity(frame_count * frame::SAMPLES_PER_CHANNEL);
        for _ in 0..frame_count {
            graph.push_frame("c1", sine_stereo_frame(440.0, 10_000, &mut phase_c1));
            graph.push_frame("c3", sine_stereo_frame(880.0, 10_000, &mut phase_c3));
            let out = graph.render_output("c2");
            left.extend(extract_channel_i16(&out.pcm, 0));
            right.extend(extract_channel_i16(&out.pcm, 1));
        }
        (left, right)
    }

    fn assert_two_sine_pan_sides(left: &[i16], right: &[i16], expect_440_on_right: bool) {
        let sample_rate = frame::SAMPLE_RATE;
        let p440_l = goertzel_power(left, sample_rate, 440.0);
        let p440_r = goertzel_power(right, sample_rate, 440.0);
        let p880_l = goertzel_power(left, sample_rate, 880.0);
        let p880_r = goertzel_power(right, sample_rate, 880.0);

        let min_ratio = 2.0;
        if expect_440_on_right {
            assert!(
                p440_r >= min_ratio * p440_l,
                "440 Hz should dominate right (L={p440_l}, R={p440_r})"
            );
            assert!(
                p440_r >= min_ratio * p880_r,
                "440 Hz on right should beat 880 Hz on right (440R={p440_r}, 880R={p880_r})"
            );
            assert!(
                p880_l >= min_ratio * p880_r,
                "880 Hz should dominate left (L={p880_l}, R={p880_r})"
            );
            assert!(
                p880_l >= min_ratio * p440_l,
                "880 Hz on left should beat 440 Hz on left (880L={p880_l}, 440L={p440_l})"
            );
        } else {
            assert!(
                p440_l >= min_ratio * p440_r,
                "440 Hz should dominate left after pose swap (L={p440_l}, R={p440_r})"
            );
            assert!(
                p440_l >= min_ratio * p880_l,
                "440 Hz on left should beat 880 Hz on left (440L={p440_l}, 880L={p880_l})"
            );
            assert!(
                p880_r >= min_ratio * p880_l,
                "880 Hz should dominate right after pose swap (L={p880_l}, R={p880_r})"
            );
            assert!(
                p880_r >= min_ratio * p440_r,
                "880 Hz on right should beat 440 Hz on right (880R={p880_r}, 440R={p440_r})"
            );
        }
    }

    #[test]
    fn positional_two_sine_sources_pan_to_correct_stereo_sides() {
        let mut graph = setup_three_client_positional_graph(3.0, -3.0);
        let (left, right) = render_listener_accumulated(&mut graph, 15);
        assert_two_sine_pan_sides(&left, &right, true);

        let mut swapped = setup_three_client_positional_graph(-3.0, 3.0);
        let (left_swapped, right_swapped) = render_listener_accumulated(&mut swapped, 15);
        assert_two_sine_pan_sides(&left_swapped, &right_swapped, false);
    }
}
