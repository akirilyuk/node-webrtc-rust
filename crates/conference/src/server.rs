//! Conference server managing multiple rooms.

use std::collections::HashMap;
use std::sync::Arc;

use node_webrtc_rust_core::debug_call;
use tokio::sync::Mutex;

use crate::error::ConferenceError;
use crate::room::{Room, RoomConfig};

/// Manages conference rooms keyed by room ID.
pub struct ConferenceServer {
    rooms: Mutex<HashMap<String, Arc<Mutex<Room>>>>,
}

impl ConferenceServer {
    /// Creates an empty conference server.
    pub fn new() -> Self {
        debug_call!("conference::server", "new");
        Self {
            rooms: Mutex::new(HashMap::new()),
        }
    }

    /// Creates a room and registers it with the server.
    pub async fn create_room(
        &self,
        room_id: &str,
        config: RoomConfig,
    ) -> Result<Arc<Mutex<Room>>, ConferenceError> {
        debug_call!(
            "conference::server",
            "create_room",
            "room_id={room_id}"
        );

        let mut rooms = self.rooms.lock().await;
        if rooms.contains_key(room_id) {
            return Err(ConferenceError::internal(format!(
                "room {room_id} already exists"
            )));
        }

        let room = Arc::new(Mutex::new(Room::new(room_id, config)));
        rooms.insert(room_id.to_owned(), Arc::clone(&room));
        Ok(room)
    }

    /// Returns a handle to an existing room.
    pub async fn get_room(&self, room_id: &str) -> Result<Arc<Mutex<Room>>, ConferenceError> {
        let rooms = self.rooms.lock().await;
        rooms
            .get(room_id)
            .cloned()
            .ok_or_else(|| ConferenceError::room_not_found(format!("room {room_id} not found")))
    }

    /// Lists active room IDs.
    pub async fn list_rooms(&self) -> Vec<String> {
        let rooms = self.rooms.lock().await;
        rooms.keys().cloned().collect()
    }

    /// Destroys a room and tears down all participants.
    pub async fn destroy_room(&self, room_id: &str) -> Result<(), ConferenceError> {
        debug_call!(
            "conference::server",
            "destroy_room",
            "room_id={room_id}"
        );

        let room = {
            let mut rooms = self.rooms.lock().await;
            rooms.remove(room_id).ok_or_else(|| {
                ConferenceError::room_not_found(format!("room {room_id} not found"))
            })?
        };

        room.lock().await.close().await?;
        Ok(())
    }
}

impl Default for ConferenceServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_get_list_destroy_room() {
        let server = ConferenceServer::new();
        let room = server
            .create_room("room-1", RoomConfig::default())
            .await
            .unwrap();
        assert_eq!(room.lock().await.id(), "room-1");

        let fetched = server.get_room("room-1").await.unwrap();
        assert!(Arc::ptr_eq(&room, &fetched));

        let ids = server.list_rooms().await;
        assert_eq!(ids, vec!["room-1".to_string()]);

        server.destroy_room("room-1").await.unwrap();
        assert!(server.get_room("room-1").await.is_err());
    }
}
