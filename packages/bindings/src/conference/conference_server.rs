//! ConferenceServer NAPI bindings.

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use node_webrtc_rust_conference::ConferenceServer;

use crate::conference::conference_room::JsConferenceRoom;
use crate::conference::types::{conference_err, JsRoomOptions};

/// Conference server managing multiple rooms.
#[napi]
pub struct JsConferenceServer {
    inner: Arc<ConferenceServer>,
}

impl Default for JsConferenceServer {
    fn default() -> Self {
        Self::new()
    }
}

#[napi]
impl JsConferenceServer {
    /// Creates an empty conference server.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ConferenceServer::new()),
        }
    }

    /// Creates a room and returns a handle to it.
    #[napi]
    pub async fn create_room(
        &self,
        room_id: String,
        options: Option<JsRoomOptions>,
    ) -> Result<JsConferenceRoom> {
        // Start OTel before the mixer caches its counter: an instrument created before the
        // global meter provider exists stays a no-op forever (voice sessions init lazily).
        #[cfg(feature = "otel")]
        let _ = node_webrtc_rust_speech::otel::init_from_env();
        let config = options.unwrap_or_default().into();
        let room = self
            .inner
            .create_room(&room_id, config)
            .await
            .map_err(conference_err)?;
        Ok(JsConferenceRoom::new(room_id, room))
    }

    /// Returns a handle to an existing room, if present.
    #[napi]
    pub async fn get_room(&self, room_id: String) -> Result<Option<JsConferenceRoom>> {
        match self.inner.get_room(&room_id).await {
            Ok(room) => Ok(Some(JsConferenceRoom::new(room_id, room))),
            Err(err)
                if err.code() == node_webrtc_rust_conference::ConferenceErrorCode::RoomNotFound =>
            {
                Ok(None)
            }
            Err(err) => Err(conference_err(err)),
        }
    }

    /// Lists active room identifiers.
    #[napi]
    pub async fn list_rooms(&self) -> Result<Vec<String>> {
        Ok(self.inner.list_rooms().await)
    }

    /// Destroys a room and tears down all participants.
    #[napi]
    pub async fn destroy_room(&self, room_id: String) -> Result<()> {
        self.inner
            .destroy_room(&room_id)
            .await
            .map_err(conference_err)
    }
}
