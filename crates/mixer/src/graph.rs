//! Per-listener mix graph with mute matrix, exclude-self routing, and positional pan.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::bus::MixBus;
use crate::elastic::{merge_pair, ElasticFrameQueue, SourceStats};
use crate::frame::{self, Frame, FRAME_MS};
use crate::otel::{record_mix_frames, MixFrameOutcome};
use crate::spatial::{
    apply_pan_gains, pan_frame_with_placement, pan_gains_placement, pan_gains_positional,
    ClientPose, DistanceParams, MixPlacement,
};

/// Participant or listener identifier.
pub type ParticipantId = String;

/// Longest catch-up after a late render (in 20 ms ticks); beyond it the media clock resyncs.
const MAX_CATCHUP_TICKS: u64 = 8;

/// Ticks of per-source output kept so a listener that renders late can still hear every
/// frame in order (it advances one tick per render until it has caught up).
///
/// A listener trails the clock by up to `MAX_CATCHUP_TICKS` plus the ticks it renders early
/// (`behind`, also capped), and merging reads one tick further back.
const HISTORY_TICKS: usize = 2 * MAX_CATCHUP_TICKS as usize + 2;

/// Which history ticks one listener render hears (see `MixGraph::next_listener_step`).
#[derive(Debug, Clone, Copy)]
enum ListenerStep {
    /// One tick, `lag` ticks behind the newest.
    Single { lag: u64 },
    /// Two consecutive ticks merged into one frame, catching up by one tick.
    Merged { older_lag: u64, newer_lag: u64 },
    /// Jumped to the newest tick; `skipped` popped ticks were not heard.
    Skipped { lag: u64, skipped: u64 },
}

/// One mix source: an elastic queue plus the frames chosen for the latest media ticks.
#[derive(Debug)]
struct SourceSlot {
    queue: ElasticFrameQueue,
    /// Output of the most recent ticks, newest last; every listener rendering in a tick
    /// mixes the same entry.
    history: VecDeque<Frame>,
    /// Popped frames merged into one output frame for a late listener (summed over listeners).
    listener_catchup_compressed: u64,
    /// Popped frames a listener jumped over (summed over listeners).
    listener_skipped: u64,
}

impl SourceSlot {
    fn new() -> Self {
        Self {
            queue: ElasticFrameQueue::new(),
            history: VecDeque::with_capacity(HISTORY_TICKS),
            listener_catchup_compressed: 0,
            listener_skipped: 0,
        }
    }

    /// Frame played `lag` ticks ago (0 = the latest tick); silence when not recorded.
    fn frame_at_lag(&self, lag: u64) -> Frame {
        usize::try_from(lag)
            .ok()
            .and_then(|lag| self.history.len().checked_sub(1 + lag))
            .and_then(|idx| self.history.get(idx))
            .cloned()
            .unwrap_or_else(frame::silence_frame)
    }
}

/// Conference mix graph: one elastic input queue per participant (advanced by a shared
/// 20 ms media clock), per-listener mute masks, optional poses, and exclusive mix groups.
#[derive(Debug)]
pub struct MixGraph {
    inputs: HashMap<ParticipantId, SourceSlot>,
    /// Time of the most recent processed media tick (set by the first push or render).
    clock: Option<Instant>,
    /// Number of media ticks processed so far.
    tick_count: u64,
    /// Per-listener position (tick number) of the next output; trails `tick_count` by at
    /// most `MAX_CATCHUP_TICKS` for a listener whose renders run late.
    listener_ticks: HashMap<ParticipantId, u64>,
    mixing_enabled: bool,
    global_mute: HashSet<ParticipantId>,
    listener_mute: HashMap<(ParticipantId, ParticipantId), bool>,
    /// When set for a listener, only these participants are mixed (allow-list).
    listener_routes: HashMap<ParticipantId, HashSet<ParticipantId>>,
    poses: HashMap<ParticipantId, ClientPose>,
    positional_enabled: bool,
    default_mix_placement: MixPlacement,
    tts_mix_placement: MixPlacement,
    /// Per-source named placement (clip inputs, etc.) when no explicit world pose is set.
    source_mix_placements: HashMap<ParticipantId, MixPlacement>,
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
            clock: None,
            tick_count: 0,
            listener_ticks: HashMap::new(),
            mixing_enabled: true,
            global_mute: HashSet::new(),
            listener_mute: HashMap::new(),
            listener_routes: HashMap::new(),
            poses: HashMap::new(),
            positional_enabled: false,
            default_mix_placement: MixPlacement::Center,
            tts_mix_placement: MixPlacement::Center,
            source_mix_placements: HashMap::new(),
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
        self.inputs.entry(id).or_insert_with(SourceSlot::new);
    }

    /// Removes a participant input slot and related mute / pose / group state.
    pub fn remove_input(&mut self, participant_id: &str) {
        self.inputs.remove(participant_id);
        self.listener_ticks.remove(participant_id);
        self.global_mute.remove(participant_id);
        self.poses.remove(participant_id);
        self.source_mix_placements.remove(participant_id);
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

    /// Named placement for a specific source (e.g. clip `play:{id}`) when it has no world pose.
    pub fn set_source_mix_placement(
        &mut self,
        participant_id: impl Into<ParticipantId>,
        placement: MixPlacement,
    ) {
        self.source_mix_placements
            .insert(participant_id.into(), placement);
    }

    /// Clears per-source named placement; default / positional rules apply again.
    pub fn clear_source_mix_placement(&mut self, participant_id: &str) {
        self.source_mix_placements.remove(participant_id);
    }

    /// Returns explicit named placement for `participant_id`, if any.
    pub fn source_mix_placement(&self, participant_id: &str) -> Option<MixPlacement> {
        self.source_mix_placements.get(participant_id).copied()
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

    /// Queues a PCM frame for a participant (arrival time = now).
    ///
    /// Frames are appended to the source's elastic queue and played one per 20 ms media
    /// tick; bunched arrivals are buffered, not overwritten. See [`crate::elastic`].
    pub fn push_frame(&mut self, participant_id: impl Into<ParticipantId>, frame: Frame) {
        self.push_frame_at(participant_id, frame, Instant::now());
    }

    /// Like [`Self::push_frame`] with an explicit arrival time (tests and simulation).
    pub fn push_frame_at(
        &mut self,
        participant_id: impl Into<ParticipantId>,
        frame: Frame,
        now: Instant,
    ) {
        if self.clock.is_none() {
            self.clock = Some(now);
        }
        let id = participant_id.into();
        self.inputs
            .entry(id)
            .or_insert_with(SourceSlot::new)
            .queue
            .push(frame, now);
    }

    /// Records an undecodable inbound packet for `participant_id` (`decode_errors` stat).
    pub fn note_decode_error(&mut self, participant_id: &str, reason: &str, now: Instant) {
        if let Some(slot) = self.inputs.get_mut(participant_id) {
            slot.queue.note_decode_error(reason, now);
        }
    }

    /// Counters and queue depth/target for one source, if registered.
    pub fn source_stats(&self, participant_id: &str) -> Option<SourceStats> {
        self.inputs.get(participant_id).map(|slot| SourceStats {
            listener_catchup_compressed: slot.listener_catchup_compressed,
            listener_skipped: slot.listener_skipped,
            ..slot.queue.stats()
        })
    }

    /// Advances the media clock to `now`: pops one frame per elapsed 20 ms tick from every
    /// source into its current snapshot (catching up when late, capped at
    /// `MAX_CATCHUP_TICKS`).
    ///
    /// A longer gap (an idle period) resyncs the clock to `now` and processes exactly one
    /// tick: frames queued meanwhile play in order on the following ticks, and any backlog
    /// is slimmed by the queue's shrink rule (counted as compressed, never skipped).
    pub fn advance_to(&mut self, now: Instant) {
        let Some(clock) = self.clock else {
            self.clock = Some(now);
            return;
        };
        let tick = Duration::from_millis(u64::from(FRAME_MS));
        let ticks = (now.saturating_duration_since(clock).as_micros() / tick.as_micros()) as u64;
        if ticks == 0 {
            return;
        }
        let run = if ticks > MAX_CATCHUP_TICKS {
            self.clock = Some(now);
            1
        } else {
            self.clock = Some(clock + tick * ticks as u32);
            ticks
        };
        for slot in self.inputs.values_mut() {
            for _ in 0..run {
                let frame = slot.queue.pop();
                if slot.history.len() == HISTORY_TICKS {
                    slot.history.pop_front();
                }
                slot.history.push_back(frame);
            }
        }
        self.tick_count += run;
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
        let gains = if self.positional_enabled && self.poses.contains_key(source_id) {
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
        } else if let Some(placement) = self.source_mix_placements.get(source_id) {
            pan_gains_placement(*placement, self.distance_params)
        } else if self.positional_enabled {
            let listener_pose = self
                .poses
                .get(listener_id)
                .copied()
                .unwrap_or_else(ClientPose::center);
            pan_gains_positional(listener_pose, ClientPose::center(), self.distance_params)
        } else {
            pan_gains_placement(self.default_mix_placement, self.distance_params)
        };
        apply_pan_gains(frame, gains)
    }

    /// Moves `listener_id` to its next output and says which history ticks it hears.
    ///
    /// A listener rendering on time trails the newest tick by exactly one and hears it
    /// ([`ListenerStep::Single`] at lag 0). One whose render ran late trails by more, and
    /// would stay that far behind forever if it stepped one tick per render, because renders
    /// come at the tick rate. So it catches up by one tick per render: the next two ticks
    /// are merged into one output frame ([`ListenerStep::Merged`], the queue's cross-fade,
    /// counted as `listener_catchup_compressed`). Only beyond `MAX_CATCHUP_TICKS` behind does
    /// it jump to the present ([`ListenerStep::Skipped`], counted as `listener_skipped`).
    ///
    /// Listeners render on their own timers, so a render at `now` can lie *before* the
    /// clock (another listener's later slot already advanced it). Such a render is
    /// `behind` whole ticks early: it may only hear ticks up to `tick_count - behind`,
    /// otherwise it would consume a tick whose time has not come and then, finding no new
    /// tick at its next slot, repeat the frame it just played.
    fn next_listener_step(&mut self, listener_id: &str, now: Instant) -> ListenerStep {
        let tick = Duration::from_millis(u64::from(FRAME_MS));
        let behind = match self.clock {
            Some(clock) if now < clock => {
                let early = clock.duration_since(now).as_micros();
                u64::try_from(early.div_ceil(tick.as_micros())).unwrap_or(MAX_CATCHUP_TICKS)
            }
            _ => 0,
        }
        .min(MAX_CATCHUP_TICKS);
        let newest = self.tick_count.saturating_sub(behind);
        let tick_count = self.tick_count;
        let cursor = self
            .listener_ticks
            .entry(listener_id.to_string())
            .or_insert(newest);
        let trailing = newest.saturating_sub(*cursor);
        if trailing <= 1 {
            // On time (or early): hear the next tick if there is one, else repeat the last.
            if trailing == 1 {
                *cursor += 1;
            }
            return ListenerStep::Single {
                lag: tick_count - *cursor,
            };
        }
        if trailing > MAX_CATCHUP_TICKS {
            let skipped = trailing - 1;
            *cursor = newest;
            return ListenerStep::Skipped {
                lag: tick_count - newest,
                skipped,
            };
        }
        *cursor += 2;
        ListenerStep::Merged {
            older_lag: tick_count - (*cursor - 1),
            newer_lag: tick_count - *cursor,
        }
    }

    /// Renders the mixed output for `listener_id` at media time `now`.
    ///
    /// Advances the media clock first (see [`Self::advance_to`]), so every listener
    /// rendering in the same 20 ms tick mixes the same per-source frame. Excludes the
    /// listener's own input, applies global and per-listener mute masks, pans each source
    /// (positional or named placement), and returns silence when mixing is disabled.
    pub fn render_output(&mut self, listener_id: &str, now: Instant) -> Frame {
        self.advance_to(now);
        let step = self.next_listener_step(listener_id, now);
        if !self.mixing_enabled {
            return frame::silence_frame();
        }

        let mut sources = Vec::new();
        let mut heard: Vec<ParticipantId> = Vec::new();
        for (participant_id, slot) in &self.inputs {
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
            let raw = match step {
                ListenerStep::Single { lag } | ListenerStep::Skipped { lag, .. } => {
                    slot.frame_at_lag(lag)
                }
                ListenerStep::Merged {
                    older_lag,
                    newer_lag,
                } => merge_pair(&slot.frame_at_lag(older_lag), &slot.frame_at_lag(newer_lag)),
            };
            sources.push(self.pan_source_for_listener(listener_id, participant_id, &raw));
            heard.push(participant_id.clone());
        }

        // Listener-level accounting: a frame is never merged or skipped uncounted.
        let (merged, skipped) = match step {
            ListenerStep::Single { .. } => (0, 0),
            ListenerStep::Merged { .. } => (1, 0),
            ListenerStep::Skipped { skipped, .. } => (0, skipped),
        };
        if merged + skipped > 0 {
            for id in &heard {
                if let Some(slot) = self.inputs.get_mut(id) {
                    slot.listener_catchup_compressed += merged;
                    slot.listener_skipped += skipped;
                }
            }
            let n = heard.len() as u64;
            if merged > 0 {
                record_mix_frames(MixFrameOutcome::ListenerCatchupCompressed, merged * n);
            }
            if skipped > 0 {
                record_mix_frames(MixFrameOutcome::ListenerSkipped, skipped * n);
            }
        }

        MixBus::mix(&sources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{Quat, Vec3};
    use bytes::Bytes;
    use std::sync::OnceLock;

    /// Shared injected clock origin: sources push at `t0()`.
    fn t0() -> Instant {
        static T0: OnceLock<Instant> = OnceLock::new();
        *T0.get_or_init(Instant::now)
    }

    /// Two media ticks after `t0()`: a lone frame has waited out its prefill and plays.
    fn ready() -> Instant {
        t0() + Duration::from_millis(2 * u64::from(frame::FRAME_MS))
    }

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

        graph.push_frame_at("alice", tone_frame(1, 1_000), t0());
        graph.push_frame_at("bob", tone_frame(2, 1_000), t0());
        graph.push_frame_at("carol", tone_frame(3, 1_000), t0());

        let out = graph.render_output("bob", ready());
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
        graph.push_frame_at("alice", tone_frame(1, 5_000), t0());
        graph.push_frame_at("bob", tone_frame(2, 5_000), t0());
        graph.push_frame_at("carol", tone_frame(3, 5_000), t0());

        let before = i16::from_le_bytes([
            graph.render_output("bob", ready()).pcm[0],
            graph.render_output("bob", ready()).pcm[1],
        ]);
        assert_eq!(before, 20_000);

        graph.set_global_mute("alice", true);

        let bob_out = i16::from_le_bytes([
            graph.render_output("bob", ready()).pcm[0],
            graph.render_output("bob", ready()).pcm[1],
        ]);
        let carol_out = i16::from_le_bytes([
            graph.render_output("carol", ready()).pcm[0],
            graph.render_output("carol", ready()).pcm[1],
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
        graph.push_frame_at("alice", tone_frame(1, 2_000), t0());
        graph.push_frame_at("bob", tone_frame(2, 2_000), t0());
        graph.push_frame_at("carol", tone_frame(3, 2_000), t0());

        graph.set_listener_mute("bob", "alice", true);

        let bob_out = graph.render_output("bob", ready());
        let carol_out = graph.render_output("carol", ready());

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
        graph.push_frame_at("alice", tone_frame(1, 10_000), t0());
        graph.push_frame_at("bob", tone_frame(2, 10_000), t0());

        graph.set_mixing_enabled(false);

        assert_eq!(
            graph.render_output("alice", ready()),
            frame::silence_frame()
        );
        assert_eq!(graph.render_output("bob", ready()), frame::silence_frame());
    }

    #[test]
    fn listener_route_allow_list_limits_mix_sources() {
        let mut graph = MixGraph::new();
        graph.add_input("alice");
        graph.add_input("bob");
        graph.add_input("carol");
        graph.push_frame_at("alice", tone_frame(1, 1_000), t0());
        graph.push_frame_at("bob", tone_frame(2, 1_000), t0());
        graph.push_frame_at("carol", tone_frame(3, 1_000), t0());

        graph.set_listener_sources("bob", &["alice".to_string()]);

        let bob_sample = i16::from_le_bytes([
            graph.render_output("bob", ready()).pcm[0],
            graph.render_output("bob", ready()).pcm[1],
        ]);
        assert_eq!(bob_sample, 1_000);

        graph.clear_listener_routes("bob");
        let bob_all = i16::from_le_bytes([
            graph.render_output("bob", ready()).pcm[0],
            graph.render_output("bob", ready()).pcm[1],
        ]);
        assert_eq!(bob_all, 4_000);
    }

    #[test]
    fn groups_isolate_abc_from_df() {
        let mut graph = MixGraph::new();
        for id in ["A", "B", "C", "D", "F"] {
            graph.add_input(id);
            graph.push_frame_at(id, mono_stereo(5_000), t0());
        }

        graph.set_group_members("g1", &["A".into(), "B".into(), "C".into()]);
        graph.set_group_members("g2", &["D".into(), "F".into()]);

        let a_out = graph.render_output("A", ready());
        let d_out = graph.render_output("D", ready());
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
            graph.push_frame_at(id, mono_stereo(8_000), t0());
        }

        graph.set_group_members("g1", &["A".into(), "B".into(), "C".into()]);
        graph.set_group_members("g2", &["D".into(), "F".into()]);

        // A hears B and C before move
        let before = graph.render_output("A", ready());
        let (before_l, _) = first_lr(&before);
        assert!(before_l > 0);

        graph.move_to_group("A", "g2");

        // A no longer hears B (not in g2 allow-list)
        graph.push_frame_at("B", mono_stereo(20_000), t0());
        let a_after = graph.render_output("A", ready());
        let (a_l, _) = first_lr(&a_after);
        // A hears D and F only
        assert!(a_l > 0);

        // D now hears A
        let d_out = graph.render_output("D", ready());
        let (d_l, _) = first_lr(&d_out);
        assert!(d_l > 0);

        // B no longer hears A — only C remains in g1 for B
        let b_out = graph.render_output("B", ready());
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

        graph.push_frame_at("source", mono_stereo(10_000), t0());
        graph.set_listener_sources("listener", &["source".to_string()]);

        let out = graph.render_output("listener", ready());
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
        graph.push_frame_at("source", mono_stereo(10_000), t0());
        graph.set_listener_sources("listener", &["source".to_string()]);
        let on = graph.render_output("listener", ready());
        let (_, r_on) = first_lr(&on);

        graph.set_positional_enabled(false);
        graph.set_default_mix_placement(MixPlacement::Left);
        let off_left = graph.render_output("listener", ready());
        let (l_off, r_off) = first_lr(&off_left);
        assert!(l_off > r_off);

        graph.set_positional_enabled(true);
        let on_again = graph.render_output("listener", ready());
        let (l_again, r_again) = first_lr(&on_again);
        assert!(r_again > l_again);
        assert!((r_again - r_on).abs() < 500);
    }

    #[test]
    fn source_mix_placement_left_pans_when_positional_off() {
        let mut graph = MixGraph::new();
        graph.add_input("listener");
        graph.add_input("source");
        graph.set_source_mix_placement("source", MixPlacement::Left);
        graph.push_frame_at("source", mono_stereo(10_000), t0());
        graph.set_listener_sources("listener", &["source".to_string()]);
        let out = graph.render_output("listener", ready());
        let (l, r) = first_lr(&out);
        assert!(l > r);
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

    fn left_only_stereo(amplitude: i16) -> Frame {
        let mut pcm = vec![0u8; frame::FRAME_BYTES];
        for i in 0..frame::SAMPLES_PER_FRAME / 2 {
            let base = i * 4;
            pcm[base..base + 2].copy_from_slice(&amplitude.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    #[test]
    fn tts_pose_right_pans_left_only_frame() {
        let mut graph = MixGraph::new();
        graph.set_positional_enabled(true);
        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        let frame = left_only_stereo(10_000);
        let panned = graph.pan_tts_frame(&frame, "listener");
        let (l, r) = first_lr(&panned);
        assert!(r > l * 3 / 2);
        assert!(r > 0);
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
    fn expired_leftover_mic_does_not_mask_tts_pan_right() {
        let mut graph = MixGraph::new();
        graph.add_input("listener");
        graph.add_input("talker");
        graph.set_positional_enabled(true);
        graph.set_listener_sources("listener", &["talker".to_string()]);
        graph.set_source_mix_placement("talker", MixPlacement::Left);
        graph.push_frame_at("talker", mono_stereo(10_000), t0());

        let (l_now, r_now) = first_lr(&graph.render_output("listener", ready()));
        assert!(l_now > r_now);

        // Four ticks later the lone frame is long gone: one faded concealment, then silence.
        let mut last = frame::silence_frame();
        for k in 1..=4u64 {
            let at = ready() + Duration::from_millis(k * u64::from(frame::FRAME_MS));
            last = graph.render_output("listener", at);
        }
        let (l_after, r_after) = first_lr(&last);
        assert_eq!(l_after, 0);
        assert_eq!(r_after, 0);

        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        let panned = graph.pan_tts_frame(&mono_stereo(10_000), "listener");
        let (tl, tr) = first_lr(&panned);
        assert!(tr > tl * 3 / 2);
    }

    #[test]
    fn frame_buffer_two_listeners_hear_same_frame_within_hold() {
        let mut graph = MixGraph::new();
        graph.add_input("listener_a");
        graph.add_input("listener_b");
        graph.add_input("talker");
        graph.set_listener_sources("listener_a", &["talker".to_string()]);
        graph.set_listener_sources("listener_b", &["talker".to_string()]);
        graph.push_frame_at("talker", mono_stereo(8_000), t0());

        let a1 = graph.render_output("listener_a", ready());
        let b1 = graph.render_output("listener_b", ready());
        assert_eq!(a1, b1);

        let (mut a2, mut b2) = (frame::silence_frame(), frame::silence_frame());
        for k in 1..=4u64 {
            let at = ready() + Duration::from_millis(k * u64::from(frame::FRAME_MS));
            a2 = graph.render_output("listener_a", at);
            b2 = graph.render_output("listener_b", at);
        }
        assert_eq!(a2, frame::silence_frame());
        assert_eq!(b2, frame::silence_frame());
    }

    #[test]
    fn tts_pose_flip_repans_dual_mono_left_after_right() {
        let mut graph = MixGraph::new();
        graph.set_positional_enabled(true);
        let frame = mono_stereo(12_000);
        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        let (l_right, r_right) = first_lr(&graph.pan_tts_frame(&frame, "listener"));
        assert!(r_right > l_right * 3 / 2);

        graph.set_tts_pose(
            "listener",
            ClientPose {
                position: Vec3::try_new(-3.0, 0.0, 0.0).unwrap(),
                orientation: Quat::IDENTITY,
            },
        );
        let (l_left, r_left) = first_lr(&graph.pan_tts_frame(&frame, "listener"));
        assert!(l_left > r_left * 3 / 2);
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
        let k =
            (0.5 + (n as f64) * f64::from(target_freq) / f64::from(sample_rate)).floor() as usize;
        let omega = (2.0 * std::f64::consts::PI * k as f64) / n as f64;
        let coeff = 2.0 * omega.cos();
        let mut s1 = 0.0f64;
        let mut s2 = 0.0f64;
        for &sample in samples {
            let x = f64::from(sample);
            let s0 = x + coeff * s1 - s2;
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
        for i in 0..frame_count {
            let now = t0() + Duration::from_millis(i as u64 * u64::from(frame::FRAME_MS));
            graph.push_frame_at("c1", sine_stereo_frame(440.0, 10_000, &mut phase_c1), now);
            graph.push_frame_at("c3", sine_stereo_frame(880.0, 10_000, &mut phase_c3), now);
            let out = graph.render_output("c2", now);
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

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn first_sample(f: &Frame) -> i16 {
        i16::from_le_bytes([f.pcm[0], f.pcm[1]])
    }

    #[test]
    fn bunched_push_reaches_listener_in_order_one_per_tick() {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        graph.set_listener_sources("listener", &["talker".to_string()]);
        // 10 frames in one instant: values 1000..=1900 (constant per frame, centre pan).
        for i in 0..10 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t0());
        }
        let mut heard = Vec::new();
        for k in 1..=10u64 {
            let out = graph.render_output("listener", t0() + ms(20 * k));
            heard.push(first_sample(&out));
        }
        let expected: Vec<i16> = (0..10)
            .map(|i| first_sample(&graph_pan(1_000 + 100 * i)))
            .collect();
        assert_eq!(heard, expected);
        let s = graph.source_stats("talker").unwrap();
        assert_eq!((s.pushed, s.played, s.compressed, s.depth), (10, 10, 0, 0));
    }

    /// First sample after the graph's default (centre) pan of a constant frame.
    fn graph_pan(amplitude: i16) -> Frame {
        let graph = MixGraph::new();
        graph.pan_source_for_listener("listener", "talker", &tone_frame(1, amplitude))
    }

    #[test]
    fn three_listeners_in_one_tick_hear_the_same_frame() {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        for l in ["a", "b", "c"] {
            graph.set_listener_sources(l, &["talker".to_string()]);
        }
        for i in 0..6 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t0());
        }
        for k in 1..=4u64 {
            let now = t0() + ms(20 * k);
            let a = graph.render_output("a", now);
            let b = graph.render_output("b", now + ms(3));
            let c = graph.render_output("c", now + ms(9));
            assert_eq!(a, b);
            assert_eq!(b, c);
        }
        // Rendering three listeners per tick did not triple the consumption.
        assert_eq!(graph.source_stats("talker").unwrap().played, 4);
    }

    #[test]
    fn late_render_catches_up_one_tick_per_elapsed_20ms() {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        graph.set_listener_sources("listener", &["talker".to_string()]);
        for i in 0..8 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t0());
        }
        graph.render_output("listener", t0() + ms(20));
        assert_eq!(graph.source_stats("talker").unwrap().played, 1);
        // 100 ms late: five ticks elapsed since the last tick.
        graph.render_output("listener", t0() + ms(120));
        assert_eq!(graph.source_stats("talker").unwrap().played, 6);
    }

    #[test]
    fn late_listener_catches_up_by_merging_never_by_skipping() {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        graph.set_listener_sources("listener", &["talker".to_string()]);
        for i in 0..8 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t0());
        }
        let mut heard = Vec::new();
        // First render on time, then the listener's timer stalls for 80 ms and fires twice
        // back-to-back (the way a delayed JS timer does), then runs on time again.
        for ms_after in [20u64, 100, 100, 120, 140] {
            let out = graph.render_output("listener", t0() + ms(ms_after));
            heard.push(first_sample(&out));
        }
        let amp = |i: i16| first_sample(&graph_pan(1_000 + 100 * i));
        // Frames 1+2 and 3+4 each come out as one merged frame (it starts with the older
        // frame); the listener is back on the live tick afterwards.
        assert_eq!(heard, vec![amp(0), amp(1), amp(3), amp(5), amp(6)]);
        let s = graph.source_stats("talker").unwrap();
        assert_eq!(s.listener_catchup_compressed, 2);
        assert_eq!(s.listener_skipped, 0);
    }

    /// Distinct, loud-enough (above the merge silence threshold) amplitude per frame index.
    fn marker_amp(k: u64) -> i16 {
        1_000 + 10 * k as i16
    }

    /// Frame indices a rendered output carries: a merged frame starts with the older frame and
    /// ends with the newer one, a single frame carries one.
    fn indices_in(out: &Frame, table: &HashMap<i16, u64>) -> Vec<u64> {
        let first = first_sample(out);
        let last = i16::from_le_bytes([out.pcm[out.pcm.len() - 2], out.pcm[out.pcm.len() - 1]]);
        let mut v: Vec<u64> = [first, last]
            .iter()
            .filter_map(|s| table.get(s).copied())
            .collect();
        v.dedup();
        v
    }

    /// Renders a paced source (frame k arrives at 20 k ms) at 20 ms slots in which the given
    /// slots are never rendered (as a JS pump that drops due slots does). Returns the indices
    /// each render carried and the final source stats.
    fn run_with_skipped_slots(
        total: u64,
        skipped_after: impl Fn(u64) -> u64,
    ) -> (Vec<Vec<u64>>, SourceStats, u64) {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        graph.set_listener_sources("listener", &["talker".to_string()]);
        let table: HashMap<i16, u64> = (0..total)
            .map(|k| (first_sample(&graph_pan(marker_amp(k))), k))
            .collect();
        let mut carried = Vec::new();
        let (mut slot, mut pushed) = (0u64, 0u64);
        while slot < total + 5 {
            slot += 1;
            slot += skipped_after(slot);
            while pushed < total && pushed <= slot {
                graph.push_frame_at(
                    "talker",
                    tone_frame(1, marker_amp(pushed)),
                    t0() + ms(20 * pushed),
                );
                pushed += 1;
            }
            let out = graph.render_output("listener", t0() + ms(20 * slot + 3));
            carried.push(indices_in(&out, &table));
        }
        let stats = graph.source_stats("talker").unwrap();
        let silent_ticks = graph.tick_count - stats.played;
        (carried, stats, silent_ticks)
    }

    #[test]
    fn skipped_pump_slots_are_merged_not_lost() {
        // Five stalls that each leave two slots unrendered (the scratch case that used to jump
        // 157 -> 166 with every counter clean).
        let (carried, stats, _) = run_with_skipped_slots(120, |slot| {
            if [20, 35, 50, 65, 80].contains(&slot) {
                2
            } else {
                0
            }
        });
        let touched: HashSet<u64> = carried.iter().flatten().copied().collect();
        let first = *touched.iter().min().unwrap();
        let last = *touched.iter().max().unwrap();
        let unheard: Vec<u64> = (first..=last).filter(|k| !touched.contains(k)).collect();
        assert!(unheard.is_empty(), "frames never heard: {unheard:?}");
        assert_eq!(stats.listener_skipped, 0);
        assert!(stats.listener_catchup_compressed >= 5, "{stats:?}");
        // Heard indices never go backwards.
        let flat: Vec<u64> = carried.iter().flatten().copied().collect();
        assert!(flat.windows(2).all(|w| w[0] <= w[1]), "{flat:?}");
    }

    #[test]
    fn pathological_stall_skips_are_counted_exactly() {
        // Six unrendered slots after every third render: the listener falls further behind
        // than a merge per render can recover, so it has to jump to the present.
        let (carried, stats, silent_ticks) =
            run_with_skipped_slots(120, |slot| if slot % 3 == 0 { 6 } else { 0 });
        assert!(
            stats.listener_skipped > 0,
            "scenario must overrun: {stats:?}"
        );
        let touched: HashSet<u64> = carried.iter().flatten().copied().collect();
        let first = *touched.iter().min().unwrap();
        let unheard = (first..120).filter(|k| !touched.contains(k)).count() as u64;
        // Every frame that was not heard is counted. The counter may exceed the unheard source
        // frames only by ticks that carried none (prefill and concealment silence, stream end).
        assert!(
            unheard <= stats.listener_skipped,
            "uncounted loss: {stats:?}"
        );
        assert!(
            stats.listener_skipped - unheard <= silent_ticks,
            "{stats:?} silent ticks {silent_ticks}"
        );
    }

    #[test]
    fn remove_input_forgets_stats_and_idle_gap_resyncs_clock() {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        graph.push_frame_at("talker", tone_frame(1, 1_000), t0());
        graph.render_output("listener", t0() + ms(2_000));
        // Capped catch-up: not 100 pops for a 2 s idle gap.
        let s = graph.source_stats("talker").unwrap();
        assert!(s.played + s.concealed <= MAX_CATCHUP_TICKS);
        graph.remove_input("talker");
        assert!(graph.source_stats("talker").is_none());
    }

    /// Idle for 500 ms after warm-up renders, then 5 frames arrive bunched and
    /// `renderer` listens paced; returns what it heard and the source stats delta.
    fn idle_then_bunched_burst(
        renderer: &str,
        warm_listener: Option<&str>,
    ) -> (Vec<i16>, SourceStats) {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        graph.set_listener_sources(renderer, &["talker".to_string()]);
        // The clock runs (warm-up renders over an empty source), then the pod goes idle.
        let warm = warm_listener.unwrap_or("warmup");
        graph.set_listener_sources(warm, &["talker".to_string()]);
        graph.render_output(warm, t0());
        graph.render_output(warm, t0() + ms(20));
        let before = graph.source_stats("talker").unwrap();
        let t1 = t0() + ms(20 + 500);
        for i in 0..5 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t1);
        }
        let mut heard = Vec::new();
        for k in 0..5u64 {
            let out = graph.render_output(renderer, t1 + ms(20 * k));
            heard.push(first_sample(&out));
        }
        let after = graph.source_stats("talker").unwrap();
        let delta = SourceStats {
            pushed: after.pushed - before.pushed,
            played: after.played - before.played,
            compressed: after.compressed - before.compressed,
            concealed: after.concealed - before.concealed,
            overflow_compressed: after.overflow_compressed - before.overflow_compressed,
            ..after
        };
        (heard, delta)
    }

    #[test]
    fn idle_gap_then_bunched_burst_new_listener_hears_every_frame() {
        let (heard, delta) = idle_then_bunched_burst("fresh", None);
        let expected: Vec<i16> = (0..5)
            .map(|i| first_sample(&graph_pan(1_000 + 100 * i)))
            .collect();
        assert_eq!(heard, expected, "all 5 frames audible, in order");
        assert_eq!(delta.played, 5, "played counts exactly the frames rendered");
        assert_eq!((delta.compressed, delta.concealed, delta.depth), (0, 0, 0));
    }

    #[test]
    fn idle_gap_then_bunched_burst_existing_listener_hears_every_frame() {
        let (heard, delta) = idle_then_bunched_burst("listener", Some("listener"));
        let expected: Vec<i16> = (0..5)
            .map(|i| first_sample(&graph_pan(1_000 + 100 * i)))
            .collect();
        assert_eq!(heard, expected, "all 5 frames audible, in order");
        assert_eq!(delta.played, 5, "played counts exactly the frames rendered");
        assert_eq!((delta.compressed, delta.concealed, delta.depth), (0, 0, 0));
    }

    #[test]
    fn idle_gap_resync_pops_one_tick_not_eight() {
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        for i in 0..12 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t0());
        }
        graph.render_output("listener", t0() + ms(2_000));
        let s = graph.source_stats("talker").unwrap();
        assert_eq!(s.played + s.concealed, 1, "resync processes one tick");
        assert_eq!(s.depth, 11);
    }

    #[test]
    fn listeners_with_different_render_phases_never_repeat_a_frame() {
        // Two listeners render 14 ms apart on their own timers. After an idle gap the
        // later-phase one ("s") resyncs the clock first; the earlier-phase one ("l") then
        // renders a slot that lies before the clock and must not play ahead of its time.
        let mut graph = MixGraph::new();
        graph.add_input("talker");
        for l in ["s", "l"] {
            graph.set_listener_sources(l, &["talker".to_string()]);
        }
        graph.render_output("s", t0());
        graph.render_output("s", t0() + ms(20));
        let t1 = t0() + ms(20 + 500);
        for i in 0..8 {
            graph.push_frame_at("talker", tone_frame(1, 1_000 + 100 * i as i16), t1);
        }
        graph.render_output("s", t1 + ms(14));
        let mut heard = Vec::new();
        for k in 0..6u64 {
            heard.push(first_sample(&graph.render_output("l", t1 + ms(20 * k))));
            // "s" is due 14 ms after each "l" slot, but "l" fires first in the next slot.
            graph.render_output("s", t1 + ms(14 + 20 * k));
        }
        let expected: Vec<i16> = std::iter::once(0)
            .chain((0..5).map(|i| first_sample(&graph_pan(1_000 + 100 * i))))
            .collect();
        assert_eq!(heard, expected, "one frame per slot, none repeated");
    }
}
