//! PeerConnection wrapper around webrtc-rs.

use std::sync::{Arc, OnceLock};

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::api::API;
use webrtc::interceptor::registry::Registry;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_gatherer_state::RTCIceGathererState;
use webrtc::ice_transport::ice_gathering_state::RTCIceGatheringState;
use webrtc::peer_connection::signaling_state::RTCSignalingState;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::offer_answer_options::{RTCAnswerOptions, RTCOfferOptions};
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::track::track_local::TrackLocal;

use crate::config::PeerConnectionConfig;
use crate::offer_answer::{AnswerOptions, OfferOptions};
use crate::data_channel::{DataChannel, DataChannelOptions};
use crate::debug_call;
use crate::debug_evt;
use crate::error::CoreError;
use crate::events::{PeerConnectionEventSenders, PeerConnectionEvents};
use crate::media::RemoteTrack;
use crate::rtp_sender::RtpSender;

/// SDP session description type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdpType {
    Offer,
    Answer,
    ProvisionalAnswer,
    Rollback,
}

impl From<RTCSdpType> for SdpType {
    fn from(value: RTCSdpType) -> Self {
        match value {
            RTCSdpType::Offer => Self::Offer,
            RTCSdpType::Answer => Self::Answer,
            RTCSdpType::Pranswer => Self::ProvisionalAnswer,
            RTCSdpType::Rollback => Self::Rollback,
            _ => Self::Offer,
        }
    }
}

impl From<SdpType> for RTCSdpType {
    fn from(value: SdpType) -> Self {
        match value {
            SdpType::Offer => Self::Offer,
            SdpType::Answer => Self::Answer,
            SdpType::ProvisionalAnswer => Self::Pranswer,
            SdpType::Rollback => Self::Rollback,
        }
    }
}

/// Session description for SDP negotiation.
#[derive(Debug, Clone)]
pub struct SessionDescription {
    pub sdp_type: SdpType,
    pub sdp: String,
}

impl SessionDescription {
    fn into_rtc(self) -> Result<RTCSessionDescription, CoreError> {
        Ok(match self.sdp_type {
            SdpType::Offer => RTCSessionDescription::offer(self.sdp)?,
            SdpType::Answer => RTCSessionDescription::answer(self.sdp)?,
            SdpType::ProvisionalAnswer => RTCSessionDescription::pranswer(self.sdp)?,
            SdpType::Rollback => {
                return Err(CoreError::InvalidState(
                    "rollback session descriptions are not supported yet".into(),
                ));
            }
        })
    }
}

impl From<RTCSessionDescription> for SessionDescription {
    fn from(desc: RTCSessionDescription) -> Self {
        Self {
            sdp_type: desc.sdp_type.into(),
            sdp: desc.sdp,
        }
    }
}

/// ICE candidate for trickle ICE.
#[derive(Debug, Clone, Default)]
pub struct IceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u16>,
    pub username_fragment: Option<String>,
}

impl IceCandidate {
    fn into_rtc(self) -> RTCIceCandidateInit {
        RTCIceCandidateInit {
            candidate: self.candidate,
            sdp_mid: self.sdp_mid,
            sdp_mline_index: self.sdp_mline_index,
            username_fragment: self.username_fragment,
        }
    }
}

impl From<RTCIceCandidateInit> for IceCandidate {
    fn from(init: RTCIceCandidateInit) -> Self {
        Self {
            candidate: init.candidate,
            sdp_mid: init.sdp_mid,
            sdp_mline_index: init.sdp_mline_index,
            username_fragment: init.username_fragment,
        }
    }
}

/// Overall peer connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

impl From<RTCPeerConnectionState> for ConnectionState {
    fn from(state: RTCPeerConnectionState) -> Self {
        match state {
            RTCPeerConnectionState::New => Self::New,
            RTCPeerConnectionState::Connecting => Self::Connecting,
            RTCPeerConnectionState::Connected => Self::Connected,
            RTCPeerConnectionState::Disconnected => Self::Disconnected,
            RTCPeerConnectionState::Failed => Self::Failed,
            RTCPeerConnectionState::Closed => Self::Closed,
            _ => Self::New,
        }
    }
}

/// ICE connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceConnectionState {
    New,
    Checking,
    Connected,
    Completed,
    Disconnected,
    Failed,
    Closed,
}

impl From<RTCIceConnectionState> for IceConnectionState {
    fn from(state: RTCIceConnectionState) -> Self {
        match state {
            RTCIceConnectionState::New => Self::New,
            RTCIceConnectionState::Checking => Self::Checking,
            RTCIceConnectionState::Connected => Self::Connected,
            RTCIceConnectionState::Completed => Self::Completed,
            RTCIceConnectionState::Disconnected => Self::Disconnected,
            RTCIceConnectionState::Failed => Self::Failed,
            RTCIceConnectionState::Closed => Self::Closed,
            _ => Self::New,
        }
    }
}

/// ICE candidate gathering state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceGatheringState {
    New,
    Gathering,
    Complete,
}

impl From<RTCIceGatheringState> for IceGatheringState {
    fn from(state: RTCIceGatheringState) -> Self {
        match state {
            RTCIceGatheringState::Gathering => Self::Gathering,
            RTCIceGatheringState::Complete => Self::Complete,
            _ => Self::New,
        }
    }
}

impl From<RTCIceGathererState> for IceGatheringState {
    fn from(state: RTCIceGathererState) -> Self {
        match state {
            RTCIceGathererState::Gathering => Self::Gathering,
            RTCIceGathererState::Complete => Self::Complete,
            _ => Self::New,
        }
    }
}

/// SDP offer/answer signaling state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalingState {
    Stable,
    HaveLocalOffer,
    HaveRemoteOffer,
    HaveLocalPranswer,
    HaveRemotePranswer,
    Closed,
}

impl From<RTCSignalingState> for SignalingState {
    fn from(state: RTCSignalingState) -> Self {
        match state {
            RTCSignalingState::Stable => Self::Stable,
            RTCSignalingState::HaveLocalOffer => Self::HaveLocalOffer,
            RTCSignalingState::HaveRemoteOffer => Self::HaveRemoteOffer,
            RTCSignalingState::HaveLocalPranswer => Self::HaveLocalPranswer,
            RTCSignalingState::HaveRemotePranswer => Self::HaveRemotePranswer,
            RTCSignalingState::Closed => Self::Closed,
            _ => Self::Stable,
        }
    }
}

/// WebRTC peer connection wrapper.
pub struct PeerConnection {
    inner: Arc<RTCPeerConnection>,
}

impl Clone for PeerConnection {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

static SHARED_API: OnceLock<Arc<API>> = OnceLock::new();

fn shared_api() -> Result<Arc<API>, CoreError> {
    if let Some(api) = SHARED_API.get() {
        return Ok(Arc::clone(api));
    }

    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine)?;

    let api = Arc::new(
        APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build(),
    );

    let _ = SHARED_API.set(Arc::clone(&api));
    Ok(api)
}

impl PeerConnection {
    /// Creates a new peer connection with the given configuration.
    pub async fn new(config: PeerConnectionConfig) -> Result<Self, CoreError> {
        config.apply_debug_override();
        debug_call!("core::peer_connection", "new", "ice_servers={}", config.ice_servers.len());
        let api = shared_api()?;
        let rtc_config = config.into_rtc_configuration();
        let pc = Arc::new(api.new_peer_connection(rtc_config).await?);

        Ok(Self { inner: pc })
    }

    /// Creates an SDP offer.
    pub async fn create_offer(
        &self,
        options: Option<OfferOptions>,
    ) -> Result<SessionDescription, CoreError> {
        let options = options.unwrap_or_default();
        if options.offer_to_receive_audio {
            self.ensure_recv_transceiver(RTPCodecType::Audio).await?;
        }
        if options.offer_to_receive_video {
            return Err(CoreError::InvalidState(
                "offerToReceiveVideo is not supported yet".into(),
            ));
        }
        let rtc_options = RTCOfferOptions {
            ice_restart: options.ice_restart,
            voice_activity_detection: options.voice_activity_detection,
        };
        let desc = self.inner.create_offer(Some(rtc_options)).await?;
        Ok(desc.into())
    }

    /// Creates an SDP answer.
    pub async fn create_answer(
        &self,
        options: Option<AnswerOptions>,
    ) -> Result<SessionDescription, CoreError> {
        let options = options.unwrap_or_default();
        let rtc_options = RTCAnswerOptions {
            voice_activity_detection: options.voice_activity_detection,
        };
        let desc = self.inner.create_answer(Some(rtc_options)).await?;
        Ok(desc.into())
    }

    async fn ensure_recv_transceiver(&self, kind: RTPCodecType) -> Result<(), CoreError> {
        let transceivers = self.inner.get_transceivers().await;
        for transceiver in transceivers {
            if transceiver.kind() == kind && transceiver.direction().has_recv() {
                return Ok(());
            }
        }
        self.inner
            .add_transceiver_from_kind(
                kind,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Recvonly,
                    send_encodings: Vec::new(),
                }),
            )
            .await?;
        Ok(())
    }

    /// Sets the local session description.
    pub async fn set_local_description(
        &self,
        desc: SessionDescription,
    ) -> Result<(), CoreError> {
        debug_call!(
            "core::peer_connection",
            "set_local_description",
            "type={:?}",
            desc.sdp_type
        );
        self.inner.set_local_description(desc.into_rtc()?).await?;
        Ok(())
    }

    /// Sets the remote session description.
    pub async fn set_remote_description(
        &self,
        desc: SessionDescription,
    ) -> Result<(), CoreError> {
        debug_call!(
            "core::peer_connection",
            "set_remote_description",
            "type={:?}",
            desc.sdp_type
        );
        self.inner.set_remote_description(desc.into_rtc()?).await?;
        Ok(())
    }

    /// Adds a trickle ICE candidate.
    pub async fn add_ice_candidate(&self, candidate: IceCandidate) -> Result<(), CoreError> {
        debug_call!(
            "core::peer_connection",
            "add_ice_candidate",
            "candidate={}",
            candidate.candidate
        );
        self.inner.add_ice_candidate(candidate.into_rtc()).await?;
        Ok(())
    }

    /// Creates a new outgoing data channel.
    pub async fn create_data_channel(
        &self,
        label: &str,
        options: Option<DataChannelOptions>,
    ) -> Result<DataChannel, CoreError> {
        debug_call!("core::peer_connection", "create_data_channel", "label={label}");
        let init = options.map(Into::into);
        let dc = self.inner.create_data_channel(label, init).await?;
        Ok(DataChannel::from_inner(dc))
    }

    /// Adds a local track to the connection.
    pub async fn add_track(
        &self,
        track: Arc<dyn TrackLocal + Send + Sync>,
    ) -> Result<RtpSender, CoreError> {
        debug_call!("core::peer_connection", "add_track");
        let sender = self.inner.add_track(track).await?;
        Ok(RtpSender::from_webrtc(sender))
    }

    /// Stops sending on the given sender (detaches the track).
    pub async fn remove_track(&self, sender: &RtpSender) -> Result<(), CoreError> {
        debug_call!("core::peer_connection", "remove_track", "sender_id={}", sender.id());
        self.inner
            .remove_track(&sender.inner())
            .await
            .map_err(|e| CoreError::Track(e.to_string()))
    }

    /// Closes the peer connection.
    pub async fn close(&self) -> Result<(), CoreError> {
        debug_call!("core::peer_connection", "close");
        self.inner.close().await?;
        Ok(())
    }

    /// Returns the current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        self.inner.connection_state().into()
    }

    /// Returns the current ICE connection state.
    pub fn ice_connection_state(&self) -> IceConnectionState {
        self.inner.ice_connection_state().into()
    }

    /// Returns the current ICE gathering state.
    pub fn ice_gathering_state(&self) -> IceGatheringState {
        self.inner.ice_gathering_state().into()
    }

    /// Returns the current signaling state.
    pub fn signaling_state(&self) -> SignalingState {
        self.inner.signaling_state().into()
    }

    /// Updates ICE servers and transport policy mid-session (W3C `setConfiguration`).
    pub async fn set_configuration(&self, config: PeerConnectionConfig) -> Result<(), CoreError> {
        debug_call!(
            "core::peer_connection",
            "set_configuration",
            "ice_servers={}",
            config.ice_servers.len()
        );
        config.apply_debug_override();
        self.inner
            .set_configuration(config.into_rtc_configuration())
            .await?;
        Ok(())
    }

    /// Returns the active configuration (copy of internal state).
    pub async fn get_configuration(&self) -> PeerConnectionConfig {
        PeerConnectionConfig::from(self.inner.get_configuration().await)
    }

    /// Triggers ICE restart and negotiation-needed (W3C `restartIce`).
    pub async fn restart_ice(&self) -> Result<(), CoreError> {
        debug_call!("core::peer_connection", "restart_ice");
        self.inner.restart_ice().await?;
        Ok(())
    }

    /// Collects WebRTC statistics as a JSON object keyed by stat id.
    pub async fn get_stats_json(&self) -> Result<String, CoreError> {
        let report = self.inner.get_stats().await;
        serde_json::to_string(&report.reports)
            .map_err(|e| CoreError::InvalidState(format!("stats serialization failed: {e}")))
    }

    /// Registers a handler for local ICE candidates.
    pub fn on_ice_candidate(&self, handler: impl Fn(Option<IceCandidate>) + Send + Sync + 'static) {
        self.inner.on_ice_candidate(Box::new(move |candidate| {
            let mapped = candidate.and_then(|c| c.to_json().ok().map(IceCandidate::from));
            debug_evt!(
                "core::peer_connection",
                "icecandidate",
                "present={}",
                mapped.is_some()
            );
            handler(mapped);
            Box::pin(async {})
        }));
    }

    /// Registers a handler for incoming remote tracks.
    pub fn on_track(&self, handler: impl Fn(RemoteTrack) + Send + Sync + 'static) {
        self.inner.on_track(Box::new(move |track, _, _| {
            let remote = RemoteTrack::from_inner(track);
            debug_evt!("core::peer_connection", "track", "id={}", remote.id());
            handler(remote);
            Box::pin(async {})
        }));
    }

    /// Registers a handler for incoming data channels.
    pub fn on_data_channel(&self, handler: impl Fn(DataChannel) + Send + Sync + 'static) {
        self.inner.on_data_channel(Box::new(move |dc| {
            let channel = DataChannel::from_inner(dc);
            debug_evt!(
                "core::peer_connection",
                "datachannel",
                "label={}",
                channel.label()
            );
            handler(channel);
            Box::pin(async {})
        }));
    }

    /// Registers a handler for connection state changes.
    pub fn on_connection_state_change(
        &self,
        handler: impl Fn(ConnectionState) + Send + Sync + 'static,
    ) {
        self.inner
            .on_peer_connection_state_change(Box::new(move |state| {
                debug_evt!("core::peer_connection", "connectionstatechange", "{state:?}");
                handler(state.into());
                Box::pin(async {})
            }));
    }

    /// Registers a handler for ICE connection state changes.
    pub fn on_ice_connection_state_change(
        &self,
        handler: impl Fn(IceConnectionState) + Send + Sync + 'static,
    ) {
        self.inner
            .on_ice_connection_state_change(Box::new(move |state| {
                debug_evt!(
                    "core::peer_connection",
                    "iceconnectionstatechange",
                    "{state:?}"
                );
                handler(state.into());
                Box::pin(async {})
            }));
    }

    /// Registers a handler for ICE gathering state changes.
    pub fn on_ice_gathering_state_change(
        &self,
        handler: impl Fn(IceGatheringState) + Send + Sync + 'static,
    ) {
        self.inner
            .on_ice_gathering_state_change(Box::new(move |state| {
                debug_evt!(
                    "core::peer_connection",
                    "icegatheringstatechange",
                    "{state:?}"
                );
                handler(state.into());
                Box::pin(async {})
            }));
    }

    /// Registers a handler for signaling state changes.
    pub fn on_signaling_state_change(
        &self,
        handler: impl Fn(SignalingState) + Send + Sync + 'static,
    ) {
        self.inner.on_signaling_state_change(Box::new(move |state| {
            debug_evt!("core::peer_connection", "signalingstatechange", "{state:?}");
            handler(state.into());
            Box::pin(async {})
        }));
    }

    /// Subscribes to all peer connection events via async channels.
    pub fn subscribe_events(&self) -> PeerConnectionEvents {
        let (senders, events) = PeerConnectionEventSenders::new();
        self.wire_event_senders(senders);
        events
    }

    fn wire_event_senders(&self, senders: PeerConnectionEventSenders) {
        let ice_tx = senders.ice_candidates;
        self.inner.on_ice_candidate(Box::new(move |candidate| {
            let mapped = candidate.and_then(|c| c.to_json().ok().map(IceCandidate::from));
            debug_evt!(
                "core::peer_connection",
                "icecandidate",
                "present={}",
                mapped.is_some()
            );
            let _ = ice_tx.send(mapped);
            Box::pin(async {})
        }));

        let track_tx = senders.tracks;
        self.inner.on_track(Box::new(move |track, _, _| {
            let remote = RemoteTrack::from_inner(track);
            debug_evt!("core::peer_connection", "track", "id={}", remote.id());
            let _ = track_tx.send(remote);
            Box::pin(async {})
        }));

        let dc_tx = senders.data_channels;
        self.inner.on_data_channel(Box::new(move |dc| {
            let channel = DataChannel::from_inner(dc);
            debug_evt!(
                "core::peer_connection",
                "datachannel",
                "label={}",
                channel.label()
            );
            let _ = dc_tx.send(channel);
            Box::pin(async {})
        }));

        let conn_tx = senders.connection_state;
        self.inner
            .on_peer_connection_state_change(Box::new(move |state| {
                debug_evt!("core::peer_connection", "connectionstatechange", "{state:?}");
                let _ = conn_tx.send(state.into());
                Box::pin(async {})
            }));

        let ice_conn_tx = senders.ice_connection_state;
        self.inner
            .on_ice_connection_state_change(Box::new(move |state| {
                debug_evt!(
                    "core::peer_connection",
                    "iceconnectionstatechange",
                    "{state:?}"
                );
                let _ = ice_conn_tx.send(state.into());
                Box::pin(async {})
            }));

        let ice_gather_tx = senders.ice_gathering_state;
        self.inner
            .on_ice_gathering_state_change(Box::new(move |state| {
                debug_evt!(
                    "core::peer_connection",
                    "icegatheringstatechange",
                    "{state:?}"
                );
                let _ = ice_gather_tx.send(state.into());
                Box::pin(async {})
            }));

        let signaling_tx = senders.signaling_state;
        self.inner.on_signaling_state_change(Box::new(move |state| {
            debug_evt!("core::peer_connection", "signalingstatechange", "{state:?}");
            let _ = signaling_tx.send(state.into());
            Box::pin(async {})
        }));

        let neg_tx = senders.negotiation_needed;
        self.inner.on_negotiation_needed(Box::new(move || {
            debug_evt!("core::peer_connection", "negotiationneeded");
            let _ = neg_tx.send(());
            Box::pin(async {})
        }));
    }

    /// Returns a promise that resolves when ICE gathering completes.
    pub async fn gathering_complete(&self) {
        let mut rx = self.inner.gathering_complete_promise().await;
        let _ = rx.recv().await;
    }

    /// Returns the current local description, if set.
    pub async fn local_description(&self) -> Option<SessionDescription> {
        self.inner.local_description().await.map(Into::into)
    }

    /// Returns the current remote description, if set.
    pub async fn remote_description(&self) -> Option<SessionDescription> {
        self.inner.remote_description().await.map(Into::into)
    }
}
