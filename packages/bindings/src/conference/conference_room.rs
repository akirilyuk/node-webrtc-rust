//! ConferenceRoom NAPI bindings.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadSafeCallContext;
use napi::JsFunction;
use napi_derive::napi;
use node_webrtc_rust_conference::{Room, SignalingMessage};
use tokio::sync::{mpsc, Mutex};

use crate::conference::events::{create_event_callback, wire_event_channel};
use crate::conference::types::{
    conference_err, JsMixingEnabledChangedEvent, JsMuteOptions, JsParticipantEvent,
    JsParticipantInfo, JsParticipantKickedEvent, JsParticipantMutedEvent, JsRoomErrorEvent,
};

struct RoomEventState {
    subscribed: bool,
    participant_joined: Option<mpsc::UnboundedReceiver<node_webrtc_rust_conference::ParticipantJoined>>,
    participant_left: Option<mpsc::UnboundedReceiver<node_webrtc_rust_conference::ParticipantLeft>>,
    participant_kicked: Option<mpsc::UnboundedReceiver<node_webrtc_rust_conference::ParticipantKicked>>,
    participant_muted: Option<mpsc::UnboundedReceiver<node_webrtc_rust_conference::ParticipantMuted>>,
    mixing_enabled_changed:
        Option<mpsc::UnboundedReceiver<node_webrtc_rust_conference::MixingEnabledChanged>>,
    error: Option<mpsc::UnboundedReceiver<node_webrtc_rust_conference::RoomError>>,
}

impl RoomEventState {
    fn new() -> Self {
        Self {
            subscribed: false,
            participant_joined: None,
            participant_left: None,
            participant_kicked: None,
            participant_muted: None,
            mixing_enabled_changed: None,
            error: None,
        }
    }

    fn subscribe(&mut self, room: &mut Room) {
        if self.subscribed {
            return;
        }

        let events = room.subscribe_events();
        self.participant_joined = Some(events.participant_joined);
        self.participant_left = Some(events.participant_left);
        self.participant_kicked = Some(events.participant_kicked);
        self.participant_muted = Some(events.participant_muted);
        self.mixing_enabled_changed = Some(events.mixing_enabled_changed);
        self.error = Some(events.error);
        self.subscribed = true;
    }
}

/// One conference room with participant and mixing controls.
#[napi]
pub struct JsConferenceRoom {
    room_id: String,
    inner: Arc<Mutex<Room>>,
    events: Arc<Mutex<RoomEventState>>,
}

impl JsConferenceRoom {
    pub(crate) fn new(room_id: String, inner: Arc<Mutex<Room>>) -> Self {
        Self {
            room_id,
            inner,
            events: Arc::new(Mutex::new(RoomEventState::new())),
        }
    }
}

#[napi]
impl JsConferenceRoom {
    /// Returns participant summaries for admin/list APIs.
    #[napi]
    pub async fn list_participants(&self) -> Result<Vec<JsParticipantInfo>> {
        let room = self.inner.lock().await;
        Ok(room
            .list_participants()
            .into_iter()
            .map(JsParticipantInfo::from)
            .collect())
    }

    /// Mutes a participant globally or for one listener.
    #[napi]
    pub async fn mute_participant(
        &self,
        target_id: String,
        options: JsMuteOptions,
    ) -> Result<()> {
        self.inner
            .lock()
            .await
            .mute_participant(
                &target_id,
                options.scope.into(),
                options.listener_id.as_deref(),
            )
            .await
            .map_err(conference_err)
    }

    /// Unmutes a participant globally or for one listener.
    #[napi]
    pub async fn unmute_participant(
        &self,
        target_id: String,
        options: JsMuteOptions,
    ) -> Result<()> {
        self.inner
            .lock()
            .await
            .unmute_participant(
                &target_id,
                options.scope.into(),
                options.listener_id.as_deref(),
            )
            .await
            .map_err(conference_err)
    }

    /// Enables or disables room-wide mixing.
    #[napi]
    pub async fn set_mixing_enabled(&self, enabled: bool) -> Result<()> {
        self.inner
            .lock()
            .await
            .set_mixing_enabled(enabled)
            .await;
        Ok(())
    }

    /// Returns whether room-wide mixing is enabled.
    #[napi]
    pub async fn is_mixing_enabled(&self) -> Result<bool> {
        Ok(self.inner.lock().await.mixing_enabled().await)
    }

    /// Removes a participant with an optional kick reason.
    #[napi]
    pub async fn kick_participant(
        &self,
        participant_id: String,
        reason: Option<String>,
    ) -> Result<()> {
        self.inner
            .lock()
            .await
            .kick_participant(&participant_id, reason.as_deref())
            .await
            .map_err(conference_err)
    }

    /// Routes a JSON signaling payload through the room.
    #[napi]
    pub async fn handle_signaling_message(&self, json: String) -> Result<String> {
        let msg = SignalingMessage::from_json(&json).map_err(conference_err)?;
        let responses = self
            .inner
            .lock()
            .await
            .handle_signaling(msg)
            .await
            .map_err(conference_err)?;
        serde_json::to_string(&responses).map_err(|err| {
            Error::from_reason(format!("failed to serialize signaling response: {err}"))
        })
    }

    /// Closes all participants and clears room state.
    #[napi]
    pub async fn close(&self) -> Result<()> {
        self.inner
            .lock()
            .await
            .close()
            .await
            .map_err(conference_err)
    }

    /// Registers a callback for participant joined events.
    #[napi]
    pub fn set_on_participant_joined(&self, env: Env, callback: JsFunction) -> Result<()> {
        let room_id = self.room_id.clone();
        let mut events = self.events.blocking_lock();
        events.subscribe(&mut self.inner.blocking_lock());
        let rx = events
            .participant_joined
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(
            &env,
            callback,
            move |ctx: ThreadSafeCallContext<node_webrtc_rust_conference::ParticipantJoined>| {
                Ok(vec![JsParticipantEvent {
                    room_id: room_id.clone(),
                    participant_id: ctx.value.participant_id,
                }])
            },
        )?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    /// Registers a callback for participant left events.
    #[napi]
    pub fn set_on_participant_left(&self, env: Env, callback: JsFunction) -> Result<()> {
        let room_id = self.room_id.clone();
        let mut events = self.events.blocking_lock();
        events.subscribe(&mut self.inner.blocking_lock());
        let rx = events
            .participant_left
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(
            &env,
            callback,
            move |ctx: ThreadSafeCallContext<node_webrtc_rust_conference::ParticipantLeft>| {
                Ok(vec![JsParticipantEvent {
                    room_id: room_id.clone(),
                    participant_id: ctx.value.participant_id,
                }])
            },
        )?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    /// Registers a callback for participant kicked events.
    #[napi]
    pub fn set_on_participant_kicked(&self, env: Env, callback: JsFunction) -> Result<()> {
        let room_id = self.room_id.clone();
        let mut events = self.events.blocking_lock();
        events.subscribe(&mut self.inner.blocking_lock());
        let rx = events
            .participant_kicked
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(
            &env,
            callback,
            move |ctx: ThreadSafeCallContext<node_webrtc_rust_conference::ParticipantKicked>| {
                Ok(vec![JsParticipantKickedEvent {
                    room_id: room_id.clone(),
                    participant_id: ctx.value.participant_id,
                    reason: ctx.value.reason,
                }])
            },
        )?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    /// Registers a callback for participant muted events.
    #[napi]
    pub fn set_on_participant_muted(&self, env: Env, callback: JsFunction) -> Result<()> {
        let room_id = self.room_id.clone();
        let mut events = self.events.blocking_lock();
        events.subscribe(&mut self.inner.blocking_lock());
        let rx = events
            .participant_muted
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(
            &env,
            callback,
            move |ctx: ThreadSafeCallContext<node_webrtc_rust_conference::ParticipantMuted>| {
                Ok(vec![JsParticipantMutedEvent {
                    room_id: room_id.clone(),
                    target_id: ctx.value.target_id,
                    scope: ctx.value.scope.into(),
                    listener_id: ctx.value.listener_id,
                }])
            },
        )?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    /// Registers a callback for mixing enabled changes.
    #[napi]
    pub fn set_on_mixing_enabled_changed(&self, env: Env, callback: JsFunction) -> Result<()> {
        let room_id = self.room_id.clone();
        let mut events = self.events.blocking_lock();
        events.subscribe(&mut self.inner.blocking_lock());
        let rx = events
            .mixing_enabled_changed
            .take()
            .expect("event receivers initialized");
        let tsfn = create_event_callback(
            &env,
            callback,
            move |ctx: ThreadSafeCallContext<node_webrtc_rust_conference::MixingEnabledChanged>| {
                Ok(vec![JsMixingEnabledChangedEvent {
                    room_id: room_id.clone(),
                    enabled: ctx.value.enabled,
                }])
            },
        )?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }

    /// Registers a callback for room error events.
    #[napi]
    pub fn set_on_error(&self, env: Env, callback: JsFunction) -> Result<()> {
        let room_id = self.room_id.clone();
        let mut events = self.events.blocking_lock();
        events.subscribe(&mut self.inner.blocking_lock());
        let rx = events.error.take().expect("event receivers initialized");
        let tsfn = create_event_callback(
            &env,
            callback,
            move |ctx: ThreadSafeCallContext<node_webrtc_rust_conference::RoomError>| {
                Ok(vec![JsRoomErrorEvent {
                    room_id: Some(room_id.clone()),
                    message: ctx.value.message,
                    code: ctx.value.code,
                }])
            },
        )?;
        wire_event_channel(rx, tsfn);
        Ok(())
    }
}
