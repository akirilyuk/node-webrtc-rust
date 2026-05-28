//! RTCRtpReceiver NAPI bindings.

use napi_derive::napi;
use node_webrtc_rust_core::{debug_call, RtpReceiver};

/// RTP receiver leg of an {@link RTCRtpTransceiver}.
#[napi]
pub struct JsRtpReceiver {
    inner: RtpReceiver,
}

#[napi]
impl JsRtpReceiver {
    #[napi(getter)]
    pub fn id(&self) -> String {
        self.inner.id().to_string()
    }

    #[napi(getter)]
    pub fn kind(&self) -> String {
        match self.inner.kind() {
            node_webrtc_rust_core::TrackKind::Audio => "audio".to_string(),
            node_webrtc_rust_core::TrackKind::Video => "video".to_string(),
        }
    }
}

impl Clone for JsRtpReceiver {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl JsRtpReceiver {
    pub(crate) fn from_receiver(receiver: RtpReceiver) -> Self {
        debug_call!("bindings::rtp_receiver", "from_receiver", "kind={}", Self::kind_string(&receiver));
        Self { inner: receiver }
    }

    fn kind_string(receiver: &RtpReceiver) -> &'static str {
        match receiver.kind() {
            node_webrtc_rust_core::TrackKind::Audio => "audio",
            node_webrtc_rust_core::TrackKind::Video => "video",
        }
    }
}
