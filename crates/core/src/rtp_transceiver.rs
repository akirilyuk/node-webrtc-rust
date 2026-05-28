//! RTP transceiver and receiver handles (Unified Plan).

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_receiver::RTCRtpReceiver;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::{RTCRtpTransceiver, RTCRtpTransceiverInit};
use webrtc::track::track_local::TrackLocal;

use crate::debug_call;
use crate::error::CoreError;
use crate::media::TrackKind;
use crate::rtp_sender::RtpSender;

static NEXT_RECEIVER_ID: AtomicU64 = AtomicU64::new(1);

/// Unified Plan transceiver direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtpTransceiverDirection {
    Sendrecv,
    Sendonly,
    Recvonly,
    Inactive,
}

impl Default for RtpTransceiverDirection {
    fn default() -> Self {
        Self::Sendrecv
    }
}

impl From<RTCRtpTransceiverDirection> for RtpTransceiverDirection {
    fn from(direction: RTCRtpTransceiverDirection) -> Self {
        match direction {
            RTCRtpTransceiverDirection::Sendrecv => Self::Sendrecv,
            RTCRtpTransceiverDirection::Sendonly => Self::Sendonly,
            RTCRtpTransceiverDirection::Recvonly => Self::Recvonly,
            RTCRtpTransceiverDirection::Inactive => Self::Inactive,
            RTCRtpTransceiverDirection::Unspecified => Self::Inactive,
        }
    }
}

impl From<RtpTransceiverDirection> for RTCRtpTransceiverDirection {
    fn from(direction: RtpTransceiverDirection) -> Self {
        match direction {
            RtpTransceiverDirection::Sendrecv => Self::Sendrecv,
            RtpTransceiverDirection::Sendonly => Self::Sendonly,
            RtpTransceiverDirection::Recvonly => Self::Recvonly,
            RtpTransceiverDirection::Inactive => Self::Inactive,
        }
    }
}

/// Options for creating a transceiver (`RTCRtpTransceiverInit` subset).
#[derive(Debug, Clone, Default)]
pub struct RtpTransceiverInit {
    pub direction: RtpTransceiverDirection,
}

impl From<RtpTransceiverInit> for RTCRtpTransceiverInit {
    fn from(init: RtpTransceiverInit) -> Self {
        Self {
            direction: init.direction.into(),
            send_encodings: Vec::new(),
        }
    }
}

/// Local track or media kind for [`PeerConnection::add_transceiver`](crate::PeerConnection::add_transceiver).
#[derive(Clone)]
pub enum TransceiverSource {
    Kind(TrackKind),
    Track(Arc<dyn TrackLocal + Send + Sync>),
}

impl fmt::Debug for TransceiverSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kind(kind) => write!(f, "Kind({kind:?})"),
            Self::Track(track) => write!(f, "Track({})", track.id()),
        }
    }
}

/// RTP receiver for one transceiver leg.
#[derive(Clone)]
pub struct RtpReceiver {
    inner: Arc<RTCRtpReceiver>,
    id: String,
}

impl RtpReceiver {
    pub(crate) fn from_webrtc(inner: Arc<RTCRtpReceiver>) -> Self {
        let id = format!("receiver-{}", NEXT_RECEIVER_ID.fetch_add(1, Ordering::Relaxed));
        Self { inner, id }
    }

    /// Opaque receiver id (stable for the lifetime of this handle).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Media kind (`audio` or `video`).
    pub fn kind(&self) -> TrackKind {
        self.inner.kind().into()
    }

    pub(crate) fn inner(&self) -> Arc<RTCRtpReceiver> {
        Arc::clone(&self.inner)
    }
}

/// Combined sender/receiver pair sharing an SDP mid.
#[derive(Clone)]
pub struct RtpTransceiver {
    inner: Arc<RTCRtpTransceiver>,
    stopped: Arc<AtomicBool>,
}

impl RtpTransceiver {
    pub(crate) fn from_webrtc(inner: Arc<RTCRtpTransceiver>) -> Self {
        Self {
            inner,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn inner(&self) -> Arc<RTCRtpTransceiver> {
        Arc::clone(&self.inner)
    }

    /// Negotiated media section mid, if assigned.
    pub fn mid(&self) -> Option<String> {
        self.inner.mid().map(|mid| mid.to_string())
    }

    /// Media kind (`audio` or `video`).
    pub fn kind(&self) -> TrackKind {
        self.inner.kind().into()
    }

    /// Desired direction before or during negotiation.
    pub fn direction(&self) -> RtpTransceiverDirection {
        self.inner.direction().into()
    }

    /// Negotiated direction; `None` when not yet negotiated or stopped.
    pub fn current_direction(&self) -> Option<RtpTransceiverDirection> {
        let direction = self.inner.current_direction();
        match direction {
            RTCRtpTransceiverDirection::Unspecified => None,
            other => Some(other.into()),
        }
    }

    /// Whether {@link Self::stop} has been called on this handle.
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Updates desired direction (may trigger negotiation-needed).
    pub async fn set_direction(&self, direction: RtpTransceiverDirection) {
        debug_call!(
            "core::rtp_transceiver",
            "set_direction",
            "kind={:?}, direction={direction:?}",
            self.kind()
        );
        self.inner.set_direction(direction.into()).await;
    }

    /// Permanently stops sending and receiving on this transceiver.
    pub async fn stop(&self) -> Result<(), CoreError> {
        debug_call!("core::rtp_transceiver", "stop", "kind={:?}", self.kind());
        self.inner.stop().await?;
        self.stopped.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Returns the sender leg of this transceiver.
    pub async fn sender(&self) -> RtpSender {
        RtpSender::from_webrtc(self.inner.sender().await)
    }

    /// Returns the receiver leg of this transceiver.
    pub async fn receiver(&self) -> RtpReceiver {
        RtpReceiver::from_webrtc(self.inner.receiver().await)
    }
}

pub(crate) fn track_kind_to_rtp(kind: TrackKind) -> RTPCodecType {
    match kind {
        TrackKind::Audio => RTPCodecType::Audio,
        TrackKind::Video => RTPCodecType::Video,
    }
}

pub fn rtp_kind_from_str(kind: &str) -> Result<TrackKind, CoreError> {
    match kind {
        "audio" => Ok(TrackKind::Audio),
        "video" => Ok(TrackKind::Video),
        other => Err(CoreError::InvalidState(format!(
            "unsupported transceiver kind: {other}"
        ))),
    }
}
