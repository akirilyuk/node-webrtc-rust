//! MediaStream and track abstractions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_remote::TrackRemote;

use crate::debug_call;
use crate::error::CoreError;
use crate::pcm_audio_track::PcmAudioTrackLocal;
use crate::pcm_encoder::NegotiatedAudioFormat;

/// Track media kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Video,
}

impl From<RTPCodecType> for TrackKind {
    fn from(kind: RTPCodecType) -> Self {
        match kind {
            RTPCodecType::Audio => Self::Audio,
            RTPCodecType::Video => Self::Video,
            _ => Self::Audio,
        }
    }
}

/// Codec information for a remote track.
#[derive(Debug, Clone)]
pub struct CodecInfo {
    pub mime_type: String,
    pub clock_rate: u32,
    pub channels: u16,
}

/// RTP packet received from a remote track.
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub payload: Bytes,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub payload_type: u8,
}

/// Common interface for local and remote media tracks.
pub trait MediaStreamTrack: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> TrackKind;
    fn stream_id(&self) -> &str;
    fn enabled(&self) -> bool;
    fn set_enabled(&self, enabled: bool);
}

/// Groups tracks by stream ID.
#[derive(Clone, Default)]
pub struct MediaStream {
    pub id: String,
    tracks: Vec<Arc<dyn MediaStreamTrack>>,
}

impl MediaStream {
    /// Creates an empty stream with the given ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            tracks: Vec::new(),
        }
    }

    /// Returns all tracks in the stream.
    pub fn tracks(&self) -> &[Arc<dyn MediaStreamTrack>] {
        &self.tracks
    }

    /// Adds a track to the stream.
    pub fn add_track(&mut self, track: Arc<dyn MediaStreamTrack>) {
        self.tracks.push(track);
    }
}

/// Local audio track: callers pass PCM; RTP uses the codec negotiated in SDP.
pub struct LocalAudioTrack {
    inner: Arc<PcmAudioTrackLocal>,
    enabled: AtomicBool,
    track_id: String,
    stream_id: String,
}

impl LocalAudioTrack {
    /// Creates a new local audio track advertising Opus (WebRTC default).
    pub fn new(id: &str, stream_id: &str) -> Self {
        Self {
            inner: Arc::new(
                PcmAudioTrackLocal::new(id, stream_id).expect("PCM audio track init"),
            ),
            enabled: AtomicBool::new(true),
            track_id: id.to_owned(),
            stream_id: stream_id.to_owned(),
        }
    }

    /// Writes interleaved stereo PCM; encoded to the negotiated RTP codec before send.
    pub async fn write_sample(&self, data: Bytes, duration: Duration) -> Result<(), CoreError> {
        self.inner.write_pcm_sample(data, duration).await
    }

    /// Writes a PCM slice, copying once into a shared buffer.
    pub async fn write_sample_slice(&self, data: &[u8], duration: Duration) -> Result<(), CoreError> {
        self.write_sample(Bytes::copy_from_slice(data), duration).await
    }

    /// Negotiated audio format after the track is bound to a peer connection.
    pub async fn negotiated_format(&self) -> Option<NegotiatedAudioFormat> {
        self.inner.negotiated_format().await
    }

    /// Returns the underlying track for use with [`PeerConnection::add_track`](crate::PeerConnection::add_track).
    pub fn as_track_local(&self) -> Arc<dyn TrackLocal + Send + Sync> {
        Arc::clone(&self.inner) as Arc<dyn TrackLocal + Send + Sync>
    }
}

impl Clone for LocalAudioTrack {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            enabled: AtomicBool::new(self.enabled.load(Ordering::SeqCst)),
            track_id: self.track_id.clone(),
            stream_id: self.stream_id.clone(),
        }
    }
}

impl MediaStreamTrack for LocalAudioTrack {
    fn id(&self) -> &str {
        &self.track_id
    }

    fn kind(&self) -> TrackKind {
        TrackKind::Audio
    }

    fn stream_id(&self) -> &str {
        &self.stream_id
    }

    fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    fn set_enabled(&self, enabled: bool) {
        debug_call!(
            "core::media",
            "set_enabled",
            "id={}, enabled={enabled}",
            self.track_id
        );
        self.enabled.store(enabled, Ordering::SeqCst);
    }
}

/// Remote track received from a peer connection.
#[derive(Clone)]
pub struct RemoteTrack {
    inner: Arc<TrackRemote>,
    track_id: String,
    stream_id: String,
}

impl RemoteTrack {
    pub(crate) fn from_inner(inner: Arc<TrackRemote>) -> Self {
        Self {
            track_id: inner.id(),
            stream_id: inner.stream_id(),
            inner,
        }
    }

    /// Shared handle to the underlying webrtc-rs track.
    pub fn inner(&self) -> Arc<TrackRemote> {
        Arc::clone(&self.inner)
    }

    /// Returns the track ID.
    pub fn id(&self) -> &str {
        &self.track_id
    }

    /// Returns the associated stream ID.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Returns the track kind.
    pub fn kind(&self) -> TrackKind {
        self.inner.kind().into()
    }

    /// Returns codec information for the track.
    pub fn codec(&self) -> CodecInfo {
        let params = self.inner.codec();
        CodecInfo {
            mime_type: params.capability.mime_type.clone(),
            clock_rate: params.capability.clock_rate,
            channels: params.capability.channels,
        }
    }

    /// Reads the next RTP packet from the track.
    pub async fn read_rtp(&self) -> Result<RtpPacket, CoreError> {
        debug_call!("core::media", "read_rtp", "id={}", self.track_id);
        let (packet, _) = self
            .inner
            .read_rtp()
            .await
            .map_err(|e| CoreError::Track(e.to_string()))?;

        Ok(RtpPacket {
            payload: packet.payload,
            sequence_number: packet.header.sequence_number,
            timestamp: packet.header.timestamp,
            payload_type: packet.header.payload_type,
        })
    }
}

impl MediaStreamTrack for RemoteTrack {
    fn id(&self) -> &str {
        &self.track_id
    }

    fn kind(&self) -> TrackKind {
        self.inner.kind().into()
    }

    fn stream_id(&self) -> &str {
        &self.stream_id
    }

    fn enabled(&self) -> bool {
        true
    }

    fn set_enabled(&self, _enabled: bool) {}
}
