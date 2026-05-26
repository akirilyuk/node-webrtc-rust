//! RTCPeerConnection NAPI bindings.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_core::{
    ConnectionState, IceConnectionState, PeerConnection, PeerConnectionEvents, RemoteTrack,
};
use tokio::sync::Mutex;

use crate::config::{
    core_err, JsRTCIceCandidate, JsRTCConfiguration, JsRTCSessionDescription,
};
use crate::data_channel::{JsRTCDataChannel, JsRTCDataChannelInit};
use crate::events::{create_event_callback, wire_event_channel};
use crate::media::JsMediaStreamTrack;

struct EventReceivers {
    events: Option<PeerConnectionEvents>,
}

/// WebRTC peer connection exposed to JavaScript.
#[napi]
pub struct JsPeerConnection {
    inner: Arc<PeerConnection>,
    events: Arc<Mutex<EventReceivers>>,
}

impl JsPeerConnection {
    fn ensure_events(&self) -> PeerConnectionEvents {
        let mut guard = self.events.blocking_lock();
        if guard.events.is_none() {
            guard.events = Some(self.inner.subscribe_events());
        }
        guard.events.take().expect("events just initialized")
    }

    fn return_events(&self, events: PeerConnectionEvents) {
        let mut guard = self.events.blocking_lock();
        guard.events = Some(events);
    }

    fn map_ice_candidate(
        env: &Env,
        candidate: Option<node_webrtc_rust_core::IceCandidate>,
    ) -> Result<Vec<Unknown<'static>>> {
        match candidate {
            None => Ok(vec![env.get_null()?.into()]),
            Some(candidate) => {
                let js = JsRTCIceCandidate::from(candidate);
                Ok(vec![js.into_unknown(env)?])
            }
        }
    }

    fn map_track(env: &Env, track: RemoteTrack) -> Result<Vec<Unknown<'static>>> {
        let js = JsMediaStreamTrack::from_remote(track);
        Ok(vec![js.into_unknown(env)?])
    }

    fn map_data_channel(env: &Env, channel: node_webrtc_rust_core::DataChannel) -> Result<Vec<Unknown<'static>>> {
        let js = JsRTCDataChannel::new(channel);
        Ok(vec![js.into_unknown(env)?])
    }

    fn map_connection_state(env: &Env, state: ConnectionState) -> Result<Vec<Unknown<'static>>> {
        Ok(vec![env
            .create_string(&connection_state_to_string(state))?
            .into()])
    }

    fn map_ice_connection_state(
        env: &Env,
        state: IceConnectionState,
    ) -> Result<Vec<Unknown<'static>>> {
        Ok(vec![env
            .create_string(&ice_connection_state_to_string(state))?
            .into()])
    }
}

#[napi]
impl JsPeerConnection {
    #[napi(constructor)]
    pub fn new(config: Option<JsRTCConfiguration>) -> Result<Self> {
        let config = config.unwrap_or_default().into();
        let inner = napi::tokio::runtime::Handle::current()
            .block_on(PeerConnection::new(config))
            .map_err(core_err)?;
        Ok(Self {
            inner: Arc::new(inner),
            events: Arc::new(Mutex::new(EventReceivers { events: None })),
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
    pub fn set_on_ice_candidate(
        &self,
        env: Env,
        callback: Function<'static, UnknownReturnValue>,
    ) -> Result<()> {
        let mut events = self.ensure_events();
        let tsfn = create_event_callback(&env, callback, Self::map_ice_candidate)?;
        wire_event_channel(events.ice_candidates, tsfn);
        self.return_events(events);
        Ok(())
    }

    #[napi]
    pub fn set_on_track(
        &self,
        env: Env,
        callback: Function<'static, UnknownReturnValue>,
    ) -> Result<()> {
        let mut events = self.ensure_events();
        let tsfn = create_event_callback(&env, callback, Self::map_track)?;
        wire_event_channel(events.tracks, tsfn);
        self.return_events(events);
        Ok(())
    }

    #[napi]
    pub fn set_on_data_channel(
        &self,
        env: Env,
        callback: Function<'static, UnknownReturnValue>,
    ) -> Result<()> {
        let mut events = self.ensure_events();
        let tsfn = create_event_callback(&env, callback, Self::map_data_channel)?;
        wire_event_channel(events.data_channels, tsfn);
        self.return_events(events);
        Ok(())
    }

    #[napi]
    pub fn set_on_connection_state_change(
        &self,
        env: Env,
        callback: Function<'static, UnknownReturnValue>,
    ) -> Result<()> {
        let mut events = self.ensure_events();
        let tsfn = create_event_callback(&env, callback, Self::map_connection_state)?;
        wire_event_channel(events.connection_state, tsfn);
        self.return_events(events);
        Ok(())
    }

    #[napi]
    pub fn set_on_ice_connection_state_change(
        &self,
        env: Env,
        callback: Function<'static, UnknownReturnValue>,
    ) -> Result<()> {
        let mut events = self.ensure_events();
        let tsfn = create_event_callback(&env, callback, Self::map_ice_connection_state)?;
        wire_event_channel(events.ice_connection_state, tsfn);
        self.return_events(events);
        Ok(())
    }
}

use node_webrtc_rust_core::SessionDescription;

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
