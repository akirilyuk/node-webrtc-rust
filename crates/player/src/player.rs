//! Clip player public API.

use std::path::Path;

use uuid::Uuid;

use crate::error::PlayerError;
use crate::registry;
use crate::source::GrowingByteWriter;
use crate::types::ClipPlayerStatus;

/// Start clip playback from a filesystem path.
pub fn play_clip_from_path(path: impl AsRef<Path>) -> Result<String, PlayerError> {
    let play_id = Uuid::new_v4().to_string();
    registry::start_from_path(play_id, path.as_ref().to_string_lossy().as_ref())
}

/// Start clip playback from a complete in-memory byte buffer.
pub fn play_clip_from_bytes(data: Vec<u8>) -> Result<String, PlayerError> {
    let play_id = Uuid::new_v4().to_string();
    registry::start_from_bytes(play_id, data)
}

/// Start progressive clip playback; append bytes via the returned writer.
pub fn play_clip_progressive() -> Result<(String, GrowingByteWriter), PlayerError> {
    let play_id = Uuid::new_v4().to_string();
    registry::start_from_growing(play_id)
}

/// Query status for a play session.
pub fn get_clip_status(play_id: &str) -> Option<ClipPlayerStatus> {
    registry::get(play_id).map(|s| s.status)
}

/// Stop playback for a play session.
pub fn stop_clip(play_id: &str) -> bool {
    registry::stop(play_id)
}

/// Crate version string (matches `CARGO_PKG_VERSION`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
