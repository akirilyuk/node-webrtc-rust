//! JavaScript type conversions for conference APIs.

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_conference::{ConferenceError, MuteScope, ParticipantInfo, RoomConfig};
use node_webrtc_rust_core::IceServer;

/// ICE server configuration for conference rooms.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsIceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
    pub credential_type: Option<String>,
}

impl From<JsIceServer> for IceServer {
    fn from(value: JsIceServer) -> Self {
        let credential_type = match value.credential_type.as_deref() {
            Some("oauth") => node_webrtc_rust_core::IceCredentialType::Oauth,
            _ => node_webrtc_rust_core::IceCredentialType::Password,
        };

        Self {
            urls: value.urls,
            username: value.username,
            credential: value.credential,
            credential_type,
        }
    }
}

/// Room creation options exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct JsRoomOptions {
    pub max_participants: Option<u32>,
    pub ice_servers: Option<Vec<JsIceServer>>,
}

impl From<JsRoomOptions> for RoomConfig {
    fn from(value: JsRoomOptions) -> Self {
        Self {
            max_participants: value.max_participants.unwrap_or(32) as usize,
            ice_servers: value
                .ice_servers
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

/// Participant summary exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsParticipantInfo {
    pub id: String,
    pub connection_state: String,
}

impl From<ParticipantInfo> for JsParticipantInfo {
    fn from(value: ParticipantInfo) -> Self {
        Self {
            id: value.id,
            connection_state: value.connection_state,
        }
    }
}

/// Mute scope matching the TypeScript `MuteScope` type.
#[napi(string_enum)]
#[derive(Debug)]
pub enum JsMuteScope {
    #[napi(value = "global")]
    Global,
    #[napi(value = "listener")]
    Listener,
}

impl From<JsMuteScope> for MuteScope {
    fn from(value: JsMuteScope) -> Self {
        match value {
            JsMuteScope::Global => MuteScope::Global,
            JsMuteScope::Listener => MuteScope::Listener,
        }
    }
}

impl From<MuteScope> for JsMuteScope {
    fn from(value: MuteScope) -> Self {
        match value {
            MuteScope::Global => JsMuteScope::Global,
            MuteScope::Listener => JsMuteScope::Listener,
        }
    }
}

/// Mute options exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMuteOptions {
    pub scope: JsMuteScope,
    pub listener_id: Option<String>,
}

/// Event payload for participant lifecycle callbacks.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsParticipantEvent {
    pub room_id: String,
    pub participant_id: String,
}

/// Event payload for participant kick callbacks.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsParticipantKickedEvent {
    pub room_id: String,
    pub participant_id: String,
    pub reason: Option<String>,
}

/// Event payload for participant mute callbacks.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsParticipantMutedEvent {
    pub room_id: String,
    pub target_id: String,
    pub scope: JsMuteScope,
    pub listener_id: Option<String>,
}

/// Event payload for mixing enabled changes.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMixingEnabledChangedEvent {
    pub room_id: String,
    pub enabled: bool,
}

/// Event payload for room errors.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsRoomErrorEvent {
    pub room_id: Option<String>,
    pub message: String,
    pub code: Option<String>,
}

pub(crate) fn conference_err(err: ConferenceError) -> Error {
    Error::from_reason(err.to_string())
}
