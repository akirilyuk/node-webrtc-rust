//! RTCRtpSender NAPI bindings.

use std::sync::Arc;

use std::sync::Mutex;

use napi::bindgen_prelude::Result;
use napi_derive::napi;
use node_webrtc_rust_core::{debug_call, LocalAudioTrack, RtpSender};

use crate::config::core_err;
use crate::media::JsLocalAudioTrack;

/// RTP sender returned from {@link RTCPeerConnection.addTrack}.
#[napi]
pub struct JsRtpSender {
    inner: RtpSender,
    track: Mutex<Arc<LocalAudioTrack>>,
}

#[napi]
impl JsRtpSender {
    #[napi(getter)]
    pub fn id(&self) -> String {
        self.inner.id().to_string()
    }

    /// Replaces the outbound audio track without renegotiation.
    #[napi]
    pub async fn replace_track(&self, track: Option<&JsLocalAudioTrack>) -> Result<()> {
        debug_call!("bindings::rtp_sender", "replace_track", "has_track={}", track.is_some());
        let local = track.map(|t| t.inner().as_track_local());
        if let Some(t) = track {
            *self.track.lock().expect("sender track lock") = t.inner();
        }
        self.inner
            .replace_track(local)
            .await
            .map_err(core_err)
    }
}

impl JsRtpSender {
    pub(crate) fn new(sender: RtpSender, track: Arc<LocalAudioTrack>) -> Self {
        Self {
            inner: sender,
            track: Mutex::new(track),
        }
    }

    pub(crate) fn inner(&self) -> &RtpSender {
        &self.inner
    }
}
