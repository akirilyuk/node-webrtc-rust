//! RTCPeerConnection NAPI bindings.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use napi::JsFunction;
use node_webrtc_rust_core::{
    ConnectionState, IceConnectionState, IceCandidate, PeerConnection, RemoteTrack,
    SessionDescription,
};
use tokio::sync::{mpsc, Mutex};

use crate::config::{
    core_err, to_js_unknown, JsRTCIceCandidate, JsRTCConfiguration, JsRTCSessionDescription,
};
use crate::data_channel::{JsRTCDataChannel, JsRTCDataChannelInit};
use crate::events::{create_event_callback, wire_event_channel};
use crate::media::{JsLocalAudioTrack, JsMediaStreamTrack};

struct EventState {
    ice_candidates: Option<mpsc::UnboundedReceiver<Option<IceCandidate>>>,
    tracks: Option<mpsc::UnboundedReceiver<RemoteTrack>>,
    data_channels: Option<mpsc::UnboundedReceiver<node_webrtc_rust_core::DataChannel>>,
    connection_state: Option<mpsc::UnboundedReceiver<ConnectionState>>,
    ice_connection_state: Option<mpsc::UnboundedReceiver<IceConnectionState>>,
}

impl EventState {
    fn new() -> Self {
        Self {
            ice_candidates: None,
            tracks: None,
            data_channels: None,
            connection_state: None,
            ice_connection_state: None,
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

    #[napi]
    pub async fn create_offer(&self) -> Result<JsRTCSessionDescription> {
        self.inner
            .create_offer()
            .await
            .map(JsRTCSessionDescription::from)
            .map_err(core_err)
    }

    #[napi]
    pub async fn create_answer(&self) -> Result<JsRTCSessionDescription> {
        self.inner
            .create_answer()
            .await
            .map(JsRTCSessionDescription::from)
            .map_err(core_err)
    }

    #[napi]
    pub async fn set_local_description(&self, desc: JsRTCSessionDescription) -> Result<()> {
        let desc = SessionDescription::try_from(desc)?;
        self.inner
            .set_local_description(desc)
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn set_remote_description(&self, desc: JsRTCSessionDescription) -> Result<()> {
        let desc = SessionDescription::try_from(desc)?;
        self.inner
            .set_remote_description(desc)
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn add_ice_candidate(&self, candidate: JsRTCIceCandidate) -> Result<()> {
        self.inner
            .add_ice_candidate(candidate.into())
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn add_track(&self, track: &JsLocalAudioTrack) -> Result<()> {
        self.inner
            .add_track(track.inner().as_track_local())
            .await
            .map_err(core_err)
    }

    #[napi]
    pub async fn create_data_channel(
        &self,
        label: String,
        options: Option<JsRTCDataChannelInit>,
    ) -> Result<JsRTCDataChannel> {
        let channel = self
            .inner
            .create_data_channel(&label, options.map(Into::into))
            .await
            .map_err(core_err)?;
        Ok(JsRTCDataChannel::new(channel))
    }

    #[napi]
    pub async fn close(&self) -> Result<()> {
        self.inner.close().await.map_err(core_err)
    }

    #[napi]
    pub async fn gathering_complete(&self) {
        self.inner.gathering_complete().await;
    }

    #[napi]
    pub async fn local_description(&self) -> Result<Option<JsRTCSessionDescription>> {
        Ok(self
            .inner
            .local_description()
            .await
            .map(JsRTCSessionDescription::from))
    }

    #[napi]
    pub async fn remote_description(&self) -> Result<Option<JsRTCSessionDescription>> {
        Ok(self
            .inner
            .remote_description()
            .await
            .map(JsRTCSessionDescription::from))
    }

    #[napi]
    pub fn set_on_ice_candidate(&self, env: Env, callback: JsFunction) -> Result<()> {
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
