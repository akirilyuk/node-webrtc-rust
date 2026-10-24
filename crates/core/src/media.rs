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
use crate::inbound_audio::stage::read_raw_rtp;
use crate::inbound_audio::{
    ImpairmentConfig, InboundAudioStageStats, InboundAudioStatsSnapshot, InboundStage,
};
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
            inner: Arc::new(PcmAudioTrackLocal::new(id, stream_id).expect("PCM audio track init")),
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
    pub async fn write_sample_slice(
        &self,
        data: &[u8],
        duration: Duration,
    ) -> Result<(), CoreError> {
        self.write_sample(Bytes::copy_from_slice(data), duration)
            .await
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

/// Decoded PCM from a remote audio track (48 kHz stereo, 20 ms by default).
#[derive(Debug, Clone)]
pub struct PcmSample {
    pub pcm: Bytes,
    pub duration: Duration,
    /// True when the audio was synthesized by loss concealment (PLC/FEC), not decoded from a
    /// received packet.
    pub concealed: bool,
}

/// Remote track received from a peer connection.
#[derive(Clone)]
pub struct RemoteTrack {
    inner: Arc<TrackRemote>,
    track_id: String,
    stream_id: String,
    stage: Arc<InboundStage>,
}

impl RemoteTrack {
    pub(crate) fn from_inner(inner: Arc<TrackRemote>) -> Self {
        let track_id = inner.id();
        Self {
            stage: Arc::new(InboundStage::new(Arc::clone(&inner), track_id.clone())),
            track_id,
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

    /// Counters of this track's inbound audio stage (reorder, loss, concealment).
    pub fn inbound_stats(&self) -> InboundAudioStatsSnapshot {
        self.stage.snapshot()
    }

    /// Shared counters of the inbound stage (for the peer connection's stats registry).
    pub(crate) fn stage_stats(&self) -> Arc<InboundAudioStageStats> {
        self.stage.stats()
    }

    /// Turns the inbound reorder/concealment stage on or off. Only effective before the
    /// first [`Self::read_sample`]. Test and diagnostics hook; default comes from
    /// `NWR_INBOUND_REORDER`.
    #[doc(hidden)]
    pub fn set_inbound_reorder_enabled(&self, on: bool) {
        self.stage.set_enabled(on);
    }

    /// Applies seeded RTP impairment (loss, bursts, reorder, duplicates) to this track's
    /// inbound packets before the stage. Only effective before the first
    /// [`Self::read_sample`]. Test hook; default comes from `NWR_TEST_RTP_IMPAIRMENT`.
    #[doc(hidden)]
    pub fn set_test_impairment(&self, cfg: Option<ImpairmentConfig>) {
        self.stage.set_impairment(cfg);
    }

    /// Reads the next PCM from the track's inbound audio stage.
    ///
    /// Packets are returned in RTP sequence order. On a clean link nothing waits. When a
    /// sequence gap appears, the stage holds later packets for an adaptive 20-60 ms, then
    /// declares the gap lost and conceals up to 100 ms of it with Opus PLC (or the next
    /// packet's in-band FEC); a packet that arrives late fills the gap if it is in time,
    /// otherwise it is dropped. Duplicates are dropped. Each received packet's audio is
    /// returned exactly once. `NWR_INBOUND_REORDER=0` restores the old path (arrival order,
    /// silence for an undecodable packet).
    ///
    /// A packet longer than 20 ms (e.g. 60 ms Opus) returns all of its audio in one sample
    /// (`pcm` is a multiple of 3 840 bytes, `duration` the matching multiple of 20 ms).
    /// A packet shorter than 20 ms returns one zero-padded frame. A concealed gap is
    /// returned as its own sample.
    pub async fn read_sample(&self) -> Result<PcmSample, CoreError> {
        self.stage.read_sample().await
    }

    /// Raw RTP in arrival order. Bypasses the inbound reorder/concealment stage; do not use
    /// together with `read_sample` on the same track.
    pub async fn read_rtp(&self) -> Result<RtpPacket, CoreError> {
        read_raw_rtp(&self.inner, &self.track_id)
            .await
            .map_err(CoreError::Track)
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
