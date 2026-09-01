//! MixGraph NAPI bindings (control plane + PCM push/render for helpers mixer).

use std::sync::Mutex;

use bytes::Bytes;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_mixer::{Frame, MixGraph, Quat, Vec3, FRAME_BYTES};

use crate::mixer::types::{
    JsClientPose, JsDistanceParams, JsMixPlacement, JsQuat, JsVec3,
};

/// Conference mix graph control handle (poses, groups, mutes, placements).
#[napi]
pub struct JsMixGraph {
    inner: Mutex<MixGraph>,
}

#[napi]
impl JsMixGraph {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(MixGraph::new()),
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
    pub fn set_tts_pose(&self, pose: JsClientPose) -> Result<()> {
        let pose = pose.try_into()?;
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .set_tts_pose(pose);
        Ok(())
    }

    #[napi]
    pub fn clear_tts_pose(&self) -> Result<()> {
        self.inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .clear_tts_pose();
        Ok(())
    }

    #[napi]
    pub fn tts_pose(&self) -> Result<Option<JsClientPose>> {
        let graph = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?;
        Ok(graph.tts_pose().map(Into::into))
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

    /// Stores the latest 20 ms stereo PCM frame for a participant (3840 bytes @ 48 kHz).
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
    #[napi]
    pub fn render_output(&self, listener_id: String) -> Result<Buffer> {
        let frame = self
            .inner
            .lock()
            .map_err(|_| Error::from_reason("mix graph lock poisoned"))?
            .render_output(&listener_id);
        Ok(Buffer::from(frame.pcm.as_ref()))
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
