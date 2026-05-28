//! RTP media sender handle returned from [`PeerConnection::add_track`](crate::PeerConnection::add_track).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::TrackLocal;

use crate::debug_call;
use crate::error::CoreError;

static NEXT_SENDER_ID: AtomicU64 = AtomicU64::new(1);

/// Local RTP sender for one negotiated media line.
#[derive(Clone)]
pub struct RtpSender {
    inner: Arc<RTCRtpSender>,
    id: String,
}

impl RtpSender {
    pub(crate) fn from_webrtc(inner: Arc<RTCRtpSender>) -> Self {
        spawn_rtcp_reader(Arc::clone(&inner));
        let id = format!("sender-{}", NEXT_SENDER_ID.fetch_add(1, Ordering::Relaxed));
        Self { inner, id }
    }

    /// Opaque sender id (stable for the lifetime of this handle).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Replaces the outbound track without renegotiation when codec-compatible.
    pub async fn replace_track(
        &self,
        track: Option<Arc<dyn TrackLocal + Send + Sync>>,
    ) -> Result<(), CoreError> {
        debug_call!("core::rtp_sender", "replace_track", "has_track={}", track.is_some());
        self.inner
            .replace_track(track)
            .await
            .map_err(|e| CoreError::Track(e.to_string()))
    }

    /// Underlying webrtc-rs sender (e.g. for [`PeerConnection::remove_track`](crate::PeerConnection::remove_track)).
    pub fn inner(&self) -> Arc<RTCRtpSender> {
        Arc::clone(&self.inner)
    }
}

fn spawn_rtcp_reader(sender: Arc<RTCRtpSender>) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 1500];
        while sender.read(&mut buf).await.is_ok() {}
    });
}
