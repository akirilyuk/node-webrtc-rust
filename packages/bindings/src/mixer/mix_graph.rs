//! MixGraph NAPI bindings (control plane + PCM push/render for helpers mixer).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_mixer::{Frame, MixGraph, Quat, Vec3, FRAME_BYTES};

use crate::mixer::types::{
    JsClientPose, JsDistanceParams, JsMixPlacement, JsMixStats, JsQuat, JsVec3,
};

/// Conference mix graph control handle (poses, groups, mutes, placements).
#[napi]
pub struct JsMixGraph {
    inner: Mutex<MixGraph>,
    /// Mapping from the caller's monotonic millisecond clock to `Instant`, fixed by the
    /// first `renderOutput(listenerId, atMs)` call: `(origin_instant, origin_ms)`.
    render_origin: Mutex<Option<(Instant, f64)>>,
}

/// Maps a caller timestamp (`at_ms`, e.g. `performance.now()`) to an `Instant` through the
/// origin set by the first call. Earlier, non-finite or unrepresentable values map to the
/// origin itself. The offset is rounded to whole microseconds so that timestamps spaced
/// exactly 20 ms apart in the caller's float domain stay exactly one media tick apart.
fn instant_for_at_ms(origin: &mut Option<(Instant, f64)>, at_ms: f64) -> Instant {
    let (origin_instant, origin_ms) = *origin.get_or_insert_with(|| (Instant::now(), at_ms));
    let delta_us = ((at_ms - origin_ms) * 1000.0).round();
    if !delta_us.is_finite() || delta_us <= 0.0 || delta_us >= u64::MAX as f64 {
        return origin_instant;
    }
    origin_instant
        .checked_add(Duration::from_micros(delta_us as u64))
        .unwrap_or(origin_instant)
}

#[napi]
impl JsMixGraph {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MixGraph::new()),
            render_origin: Mutex::new(None),
        }
    }

    #[napi]
    pub fn add_input(&self, participant_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .add_input(participant_id);
        Ok(())
    }

    #[napi]
    pub fn remove_input(&self, participant_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .remove_input(&participant_id);
        Ok(())
    }

    #[napi]
    pub fn set_mixing_enabled(&self, enabled: bool) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_mixing_enabled(enabled);
        Ok(())
    }

    #[napi]
    pub fn mixing_enabled(&self) -> Result<bool> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .mixing_enabled())
    }

    #[napi]
    pub fn set_global_mute(&self, target: String, muted: bool) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_global_mute(target, muted);
        Ok(())
    }

    #[napi]
    pub fn is_globally_muted(&self, target: String) -> Result<bool> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .is_globally_muted(&target))
    }

    #[napi]
    pub fn set_listener_mute(&self, listener: String, target: String, muted: bool) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_listener_mute(listener, target, muted);
        Ok(())
    }

    #[napi]
    pub fn is_listener_muted(&self, listener: String, target: String) -> Result<bool> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .is_listener_muted(&listener, &target))
    }

    #[napi]
    pub fn set_listener_sources(&self, listener: String, sources: Vec<String>) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_listener_sources(listener, &sources);
        Ok(())
    }

    #[napi]
    pub fn listener_sources(&self, listener: String) -> Result<Option<Vec<String>>> {
        let graph = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?;
        Ok(graph
            .listener_sources(&listener)
            .map(|set| set.iter().cloned().collect()))
    }

    #[napi]
    pub fn clear_listener_routes(&self, listener: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .clear_listener_routes(&listener);
        Ok(())
    }

    #[napi]
    pub fn set_pose(&self, participant_id: String, pose: JsClientPose) -> Result<()> {
        let pose = pose.try_into()?;
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_pose(participant_id, pose);
        Ok(())
    }

    #[napi]
    pub fn clear_pose(&self, participant_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .clear_pose(&participant_id);
        Ok(())
    }

    #[napi]
    pub fn pose(&self, participant_id: String) -> Result<Option<JsClientPose>> {
        let graph = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?;
        Ok(graph.pose(&participant_id).map(Into::into))
    }

    #[napi]
    pub fn set_positional_enabled(&self, enabled: bool) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_positional_enabled(enabled);
        Ok(())
    }

    #[napi]
    pub fn positional_enabled(&self) -> Result<bool> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .positional_enabled())
    }

    #[napi]
    pub fn set_default_mix_placement(&self, placement: JsMixPlacement) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_default_mix_placement(placement.into());
        Ok(())
    }

    #[napi]
    pub fn default_mix_placement(&self) -> Result<JsMixPlacement> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .default_mix_placement()
            .into())
    }

    #[napi]
    pub fn set_tts_mix_placement(&self, placement: JsMixPlacement) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_tts_mix_placement(placement.into());
        Ok(())
    }

    #[napi]
    pub fn tts_mix_placement(&self) -> Result<JsMixPlacement> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .tts_mix_placement()
            .into())
    }

    #[napi]
    pub fn set_source_mix_placement(
        &self,
        participant_id: String,
        placement: JsMixPlacement,
    ) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_source_mix_placement(participant_id, placement.into());
        Ok(())
    }

    #[napi]
    pub fn clear_source_mix_placement(&self, participant_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .clear_source_mix_placement(&participant_id);
        Ok(())
    }

    #[napi]
    pub fn source_mix_placement(&self, participant_id: String) -> Result<Option<JsMixPlacement>> {
        let graph = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?;
        Ok(graph.source_mix_placement(&participant_id).map(Into::into))
    }

    #[napi]
    pub fn set_tts_pose(&self, participant_id: String, pose: JsClientPose) -> Result<()> {
        let pose = pose.try_into()?;
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_tts_pose(participant_id, pose);
        Ok(())
    }

    #[napi]
    pub fn clear_tts_pose(&self, participant_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .clear_tts_pose(&participant_id);
        Ok(())
    }

    #[napi]
    pub fn tts_pose(&self, participant_id: String) -> Result<Option<JsClientPose>> {
        let graph = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?;
        Ok(graph.tts_pose(&participant_id).map(Into::into))
    }

    #[napi]
    pub fn set_distance_params(&self, params: JsDistanceParams) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_distance_params(params.into());
        Ok(())
    }

    #[napi]
    pub fn distance_params(&self) -> Result<JsDistanceParams> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .distance_params()
            .into())
    }

    #[napi]
    pub fn set_group_members(&self, group_id: String, members: Vec<String>) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_group_members(group_id, &members);
        Ok(())
    }

    #[napi]
    pub fn move_to_group(&self, participant_id: String, group_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .move_to_group(participant_id, group_id);
        Ok(())
    }

    #[napi]
    pub fn remove_from_group(&self, participant_id: String) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .remove_from_group(&participant_id);
        Ok(())
    }

    /// Queues a 20 ms stereo PCM frame for a participant (3840 bytes @ 48 kHz). Frames are
    /// played one per 20 ms render tick; bunched pushes are buffered, not overwritten.
    #[napi]
    pub fn push_frame(&self, participant_id: String, pcm: Buffer) -> Result<()> {
        if pcm.len() != FRAME_BYTES {
            return Err(Error::from_reason(format!(
                "mix pushFrame expects {FRAME_BYTES} bytes (48 kHz stereo 20 ms), got {}",
                pcm.len()
            )));
        }
        let frame = Frame::new(Bytes::copy_from_slice(pcm.as_ref()), None);
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .push_frame(participant_id, frame);
        Ok(())
    }

    /// Renders mixed stereo PCM for `listener_id` (3840 bytes; silence when ungrouped or mixing off).
    ///
    /// `at_ms` is the scheduled render time on the caller's monotonic millisecond clock
    /// (`performance.now()`): listeners rendered with the same value in one slot hear the same
    /// tick, and a catch-up render at `at_ms + 20` plays the next frame instead of repeating
    /// the previous one. Without it the render time is the moment of the call.
    #[napi]
    pub fn render_output(&self, listener_id: String, at_ms: Option<f64>) -> Result<Buffer> {
        let now = match at_ms {
            Some(at_ms) => {
                let mut origin = self
                    .render_origin
                    .lock()
                    .map_err(|_| Error::from_reason("mix graph lock poisoned"))?;
                instant_for_at_ms(&mut origin, at_ms)
            }
            None => Instant::now(),
        };
        let frame = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .render_output(&listener_id, now);
        Ok(Buffer::from(frame.pcm.as_ref()))
    }

    /// Queue counters and depth/target for one source, or `null` when it is not registered.
    #[napi]
    pub fn mix_stats(&self, participant_id: String) -> Result<Option<JsMixStats>> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .source_stats(&participant_id)
            .map(Into::into))
    }

    /// Pans a TTS frame for `listener_id` using the graph's TTS pose or placement.
    #[napi]
    pub fn pan_tts_frame(&self, pcm: Buffer, listener_id: String) -> Result<Buffer> {
        if pcm.len() != FRAME_BYTES {
            return Err(Error::from_reason(format!(
                "mix panTtsFrame expects {FRAME_BYTES} bytes (48 kHz stereo 20 ms), got {}",
                pcm.len()
            )));
        }
        let frame = Frame::new(Bytes::copy_from_slice(pcm.as_ref()), None);
        let out = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .pan_tts_frame(&frame, &listener_id);
        Ok(Buffer::from(out.pcm.as_ref()))
    }
}

impl Default for JsMixGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// Identity quaternion helper for pose setup from TypeScript.
#[napi]
pub fn js_quat_identity() -> JsQuat {
    Quat::IDENTITY.into()
}

/// Origin position helper for pose setup from TypeScript.
#[napi]
pub fn js_vec3_zero() -> JsVec3 {
    Vec3::ZERO.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_ms_maps_to_origin_plus_offset() {
        let mut origin = None;
        let t0 = instant_for_at_ms(&mut origin, 5_000.25);
        assert_eq!(
            instant_for_at_ms(&mut origin, 5_000.25),
            t0,
            "same timestamp, same instant"
        );
        assert_eq!(
            instant_for_at_ms(&mut origin, 5_020.25),
            t0 + Duration::from_millis(20)
        );
    }

    #[test]
    fn at_ms_slots_stay_exactly_one_tick_apart_despite_float_noise() {
        let mut origin = None;
        let base = 123_456.789_012_345_f64;
        let t0 = instant_for_at_ms(&mut origin, base);
        for n in 1..2_000u64 {
            let at = instant_for_at_ms(&mut origin, base + n as f64 * 20.0);
            assert_eq!(at - t0, Duration::from_millis(20 * n), "slot {n}");
        }
    }

    #[test]
    fn at_ms_earlier_or_non_finite_maps_to_origin() {
        let mut origin = None;
        let t0 = instant_for_at_ms(&mut origin, 1_000.0);
        assert_eq!(instant_for_at_ms(&mut origin, 10.0), t0);
        assert_eq!(instant_for_at_ms(&mut origin, f64::NAN), t0);
        assert_eq!(instant_for_at_ms(&mut origin, f64::INFINITY), t0);
        assert_eq!(instant_for_at_ms(&mut origin, f64::NEG_INFINITY), t0);
        assert_eq!(instant_for_at_ms(&mut origin, 1e300), t0);
    }
}
