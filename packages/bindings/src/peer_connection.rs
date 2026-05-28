//! RTCPeerConnection NAPI bindings.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use napi::JsFunction;
use node_webrtc_rust_core::{
    ConnectionState, IceConnectionState, IceCandidate, PeerConnection, RemoteTrack,
    SessionDescription, debug_call,
};
use tokio::sync::{mpsc, Mutex};

use crate::config::{
    core_err, to_js_unknown, JsRTCIceCandidate, JsRTCConfiguration, JsRTCSessionDescription,
};
use crate::data_channel::{JsRTCDataChannel, JsRTCDataChannelInit};
use crate::events::{create_event_callback, create_void_callback, wire_event_channel, wire_void_channel};
use crate::media::{JsLocalAudioTrack, JsMediaStreamTrack};
use crate::rtp_sender::JsRtpSender;

struct EventState {
    ice_candidates: Option<mpsc::UnboundedReceiver<Option<IceCandidate>>>,
    tracks: Option<mpsc::UnboundedReceiver<RemoteTrack>>,
    data_channels: Option<mpsc::UnboundedReceiver<node_webrtc_rust_core::DataChannel>>,
    connection_state: Option<mpsc::UnboundedReceiver<ConnectionState>>,
    ice_connection_state: Option<mpsc::UnboundedReceiver<IceConnectionState>>,
    negotiation_needed: Option<mpsc::UnboundedReceiver<()>>,
}

impl EventState {
    fn new() -> Self {
        Self {
            ice_candidates: None,
            tracks: None,
            data_channels: None,
            connection_state: None,
            ice_connection_state: None,
            negotiation_needed: None,
        }
    }

    fn subscribe(&mut self, pc: &PeerConnection) {
        if self.ice_candidates.is_some() {
            return;
        }

        let events = pc.subscribe_events();
        self.ice_candidates = Some(events.ice_candidates);
        self.tracks = Some(events.tracks);
        self.data_channels = Some(events.data_channels);
        self.connection_state = Some(events.connection_state);
        self.ice_connection_state = Some(events.ice_connection_state);
        self.negotiation_needed = Some(events.negotiation_needed);
    }
}

/// WebRTC peer connection exposed to JavaScript.
#[napi]
pub struct JsPeerConnection {
    inner: Arc<PeerConnection>,
    events: Arc<Mutex<EventState>>,
}

#[napi]
impl JsPeerConnection {
    #[napi(constructor)]
    pub fn new(config: Option<JsRTCConfiguration>) -> Result<Self> {
        debug_call!("bindings::peer_connection", "new");
        let config = config.unwrap_or_default().into();
        let inner = block_on(PeerConnection::new(config)).map_err(core_err)?;
        Ok(Self {
            inner: Arc::new(inner),
            events: Arc::new(Mutex::new(EventState::new())),
        })
    }

    #[napi(getter)]
    pub fn connection_state(&self) -> String {
        connection_state_to_string(self.inner.connection_state())
    }

    #[napi(getter)]
    pub fn ice_connection_state(&self) -> String {
        ice_connection_state_to_string(self.inner.ice_connection_state())
    }

    #[napi(getter)]
    pub fn ice_gathering_state(&self) -> String {
        ice_gathering_state_to_string(self.inner.ice_gathering_state())
    }

    #[napi(getter)]
    pub fn signaling_state(&self) -> String {
        signaling_state_to_string(self.inner.signaling_state())
    }

    #[napi]
    pub async fn create_offer(&self) -> Result<JsRTCSessionDescription> {
        debug_call!("bindings::peer_connection", "create_offer");
        self.inner
            .create_offer()
            .await
            .map(JsRTCSessionDescription::from)
            .map_err(core_err)
    }

    #[napi]
    pub async fn create_answer(&self) -> Result<JsRTCSessionDescription> {
        debug_call!("bindings::peer_connection", "create_answer");
        self.inner
            .create_answer()
            .await
            .map(JsRTCSessionDescription::from)
            .map_err(core_err)
    }

    #[napi]
    pub async fn set_local_description(&self, desc: JsRTCSessionDescription) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_local_description", "type={}", desc.r#type);
        let desc = SessionDescription::try_from(desc)?;
        self.inner
            .set_local_description(desc)
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn set_remote_description(&self, desc: JsRTCSessionDescription) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_remote_description", "type={}", desc.r#type);
        let desc = SessionDescription::try_from(desc)?;
        self.inner
            .set_remote_description(desc)
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn add_ice_candidate(&self, candidate: JsRTCIceCandidate) -> Result<()> {
        debug_call!(
            "bindings::peer_connection",
            "add_ice_candidate",
            "candidate={}",
            candidate.candidate
        );
        self.inner
            .add_ice_candidate(candidate.into())
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn add_track(&self, track: &JsLocalAudioTrack) -> Result<JsRtpSender> {
        debug_call!("bindings::peer_connection", "add_track");
        let inner_track = track.inner();
        let sender = self
            .inner
            .add_track(inner_track.as_track_local())
            .await
            .map_err(core_err)?;
        Ok(JsRtpSender::new(sender, inner_track))
    }

    #[napi]
    pub async fn create_data_channel(
        &self,
        label: String,
        options: Option<JsRTCDataChannelInit>,
    ) -> Result<JsRTCDataChannel> {
        debug_call!("bindings::peer_connection", "create_data_channel", "label={label}");
        let channel = self
            .inner
            .create_data_channel(&label, options.map(Into::into))
            .await
            .map_err(core_err)?;
        Ok(JsRTCDataChannel::new(channel))
    }

    #[napi]
    pub async fn close(&self) -> Result<()> {
        debug_call!("bindings::peer_connection", "close");
        self.inner.close().await.map_err(core_err)
    }

    #[napi]
    pub async fn gathering_complete(&self) {
        debug_call!("bindings::peer_connection", "gathering_complete");
        self.inner.gathering_complete().await;
    }

    #[napi]
    pub async fn local_description(&self) -> Result<Option<JsRTCSessionDescription>> {
        debug_call!("bindings::peer_connection", "local_description");
        Ok(self
            .inner
            .local_description()
            .await
            .map(JsRTCSessionDescription::from))
    }

    #[napi]
    pub async fn remote_description(&self) -> Result<Option<JsRTCSessionDescription>> {
        debug_call!("bindings::peer_connection", "remote_description");
        Ok(self
            .inner
            .remote_description()
            .await
            .map(JsRTCSessionDescription::from))
    }

    #[napi]
    pub fn set_on_ice_candidate(&self, env: Env, callback: JsFunction) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_on_ice_candidate");
        let mut events = self.events.blocking_lock();
        events.subscribe(&self.inner);
        let rx = events
            .ice_candidates
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(&env, callback, |ctx| -> Result<Vec<napi::JsUnknown>> {
            match ctx.value {
                None => to_js_unknown(&ctx.env, ctx.env.get_null()?).map(|value| vec![value]),
                Some(candidate) => {
                    to_js_unknown(&ctx.env, JsRTCIceCandidate::from(candidate)).map(|value| vec![value])
                }
            }
        })?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    #[napi]
    pub fn set_on_track(&self, env: Env, callback: JsFunction) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_on_track");
        let mut events = self.events.blocking_lock();
        events.subscribe(&self.inner);
        let rx = events.tracks.take().expect("event receivers initialized");
        let tsfn = create_event_callback(&env, callback, |ctx| -> Result<Vec<JsMediaStreamTrack>> {
            Ok(vec![JsMediaStreamTrack::from_remote(ctx.value)])
        })?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    #[napi]
    pub fn set_on_data_channel(&self, env: Env, callback: JsFunction) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_on_data_channel");
        let mut events = self.events.blocking_lock();
        events.subscribe(&self.inner);
        let rx = events
            .data_channels
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(&env, callback, |ctx| -> Result<Vec<JsRTCDataChannel>> {
            Ok(vec![JsRTCDataChannel::new(ctx.value)])
        })?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    #[napi]
    pub fn set_on_connection_state_change(&self, env: Env, callback: JsFunction) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_on_connection_state_change");
        let mut events = self.events.blocking_lock();
        events.subscribe(&self.inner);
        let rx = events
            .connection_state
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(&env, callback, |ctx| {
            Ok(vec![
                ctx.env
                    .create_string(&connection_state_to_string(ctx.value))?,
            ])
        })?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    #[napi]
    pub fn set_on_ice_connection_state_change(&self, env: Env, callback: JsFunction) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_on_ice_connection_state_change");
        let mut events = self.events.blocking_lock();
        events.subscribe(&self.inner);
        let rx = events
            .ice_connection_state
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(&env, callback, |ctx| {
            Ok(vec![
                ctx.env
                    .create_string(&ice_connection_state_to_string(ctx.value))?,
            ])
        })?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    #[napi]
    pub fn set_on_negotiation_needed(&self, env: Env, callback: JsFunction) -> Result<()> {
        debug_call!("bindings::peer_connection", "set_on_negotiation_needed");
        let mut events = self.events.blocking_lock();
        events.subscribe(&self.inner);
        let rx = events
            .negotiation_needed
            .take()
            .expect("event receivers initialized");
        let tsfn = create_void_callback(&env, callback)?;
        wire_void_channel(rx, tsfn);
        Ok(())
    }
}

fn connection_state_to_string(state: ConnectionState) -> String {
    match state {
        ConnectionState::New => "new".to_string(),
        ConnectionState::Connecting => "connecting".to_string(),
        ConnectionState::Connected => "connected".to_string(),
        ConnectionState::Disconnected => "disconnected".to_string(),
        ConnectionState::Failed => "failed".to_string(),
        ConnectionState::Closed => "closed".to_string(),
    }
}

fn ice_connection_state_to_string(state: IceConnectionState) -> String {
    match state {
        IceConnectionState::New => "new".to_string(),
        IceConnectionState::Checking => "checking".to_string(),
        IceConnectionState::Connected => "connected".to_string(),
        IceConnectionState::Completed => "completed".to_string(),
        IceConnectionState::Disconnected => "disconnected".to_string(),
        IceConnectionState::Failed => "failed".to_string(),
        IceConnectionState::Closed => "closed".to_string(),
    }
}

fn ice_gathering_state_to_string(state: node_webrtc_rust_core::IceGatheringState) -> String {
    match state {
        node_webrtc_rust_core::IceGatheringState::Gathering => "gathering".to_string(),
        node_webrtc_rust_core::IceGatheringState::Complete => "complete".to_string(),
        node_webrtc_rust_core::IceGatheringState::New => "new".to_string(),
    }
}

fn signaling_state_to_string(state: node_webrtc_rust_core::SignalingState) -> String {
    match state {
        node_webrtc_rust_core::SignalingState::Stable => "stable".to_string(),
        node_webrtc_rust_core::SignalingState::HaveLocalOffer => "have-local-offer".to_string(),
        node_webrtc_rust_core::SignalingState::HaveRemoteOffer => "have-remote-offer".to_string(),
        node_webrtc_rust_core::SignalingState::HaveLocalPranswer => "have-local-pranswer".to_string(),
        node_webrtc_rust_core::SignalingState::HaveRemotePranswer => {
            "have-remote-pranswer".to_string()
        }
        node_webrtc_rust_core::SignalingState::Closed => "closed".to_string(),
    }
}
