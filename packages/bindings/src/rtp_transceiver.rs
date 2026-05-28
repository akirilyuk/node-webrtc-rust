//! RTCRtpTransceiver NAPI bindings.

use napi::bindgen_prelude::Result;
use napi_derive::napi;
use node_webrtc_rust_core::{
    debug_call, RtpTransceiver, RtpTransceiverDirection, RtpTransceiverInit,
};

use crate::config::core_err;
use crate::rtp_receiver::JsRtpReceiver;
use crate::rtp_sender::JsRtpSender;

/// Init options for {@link RTCPeerConnection.addTransceiver}.
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRTCRtpTransceiverInit {
    pub direction: Option<String>,
}

pub(crate) fn transceiver_init_from_js(value: Option<JsRTCRtpTransceiverInit>) -> RtpTransceiverInit {
    let value = value.unwrap_or_default();
    RtpTransceiverInit {
        direction: direction_from_js(value.direction),
    }
}

fn direction_from_js(value: Option<String>) -> RtpTransceiverDirection {
    match value.as_deref() {
        Some("sendonly") => RtpTransceiverDirection::Sendonly,
        Some("recvonly") => RtpTransceiverDirection::Recvonly,
        Some("inactive") => RtpTransceiverDirection::Inactive,
        _ => RtpTransceiverDirection::Sendrecv,
    }
}

fn direction_to_js(direction: RtpTransceiverDirection) -> String {
    match direction {
        RtpTransceiverDirection::Sendrecv => "sendrecv".to_string(),
        RtpTransceiverDirection::Sendonly => "sendonly".to_string(),
        RtpTransceiverDirection::Recvonly => "recvonly".to_string(),
        RtpTransceiverDirection::Inactive => "inactive".to_string(),
    }
}

/// Unified Plan transceiver (sender + receiver pair).
#[napi]
pub struct JsRtpTransceiver {
    inner: RtpTransceiver,
    sender: JsRtpSender,
    receiver: JsRtpReceiver,
}

#[napi]
impl JsRtpTransceiver {
    #[napi(getter)]
    pub fn mid(&self) -> Option<String> {
        self.inner.mid()
    }

    #[napi(getter)]
    pub fn direction(&self) -> String {
        direction_to_js(self.inner.direction())
    }

    #[napi(getter)]
    pub fn current_direction(&self) -> Option<String> {
        self.inner.current_direction().map(direction_to_js)
    }

    #[napi(getter)]
    pub fn kind(&self) -> String {
        match self.inner.kind() {
            node_webrtc_rust_core::TrackKind::Audio => "audio".to_string(),
            node_webrtc_rust_core::TrackKind::Video => "video".to_string(),
        }
    }

    #[napi(getter)]
    pub fn stopped(&self) -> bool {
        self.inner.stopped()
    }

    #[napi(getter)]
    pub fn sender(&self) -> JsRtpSender {
        self.sender.clone()
    }

    #[napi(getter)]
    pub fn receiver(&self) -> JsRtpReceiver {
        self.receiver.clone()
    }

    #[napi]
    pub async fn set_direction(&self, direction: String) -> Result<()> {
        debug_call!("bindings::rtp_transceiver", "set_direction", "direction={direction}");
        self.inner
            .set_direction(direction_from_js(Some(direction)))
            .await;
        Ok(())
    }

    #[napi]
    pub async fn stop(&self) -> Result<()> {
        debug_call!("bindings::rtp_transceiver", "stop");
        self.inner.stop().await.map_err(core_err)
    }
}

impl JsRtpTransceiver {
    pub(crate) async fn from_transceiver(
        transceiver: RtpTransceiver,
        track: Option<std::sync::Arc<node_webrtc_rust_core::LocalAudioTrack>>,
    ) -> Self {
        let sender = transceiver.sender().await;
        let receiver = transceiver.receiver().await;
        Self {
            inner: transceiver,
            sender: match track {
                Some(t) => JsRtpSender::new(sender, t),
                None => JsRtpSender::from_sender(sender),
            },
            receiver: JsRtpReceiver::from_receiver(receiver),
        }
    }
}
