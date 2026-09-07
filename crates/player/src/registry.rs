//! Global registry of active clip play sessions.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::error::PlayerError;
use crate::session::ClipSession;

static REGISTRY: OnceLock<Mutex<HashMap<String, ClipSession>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, ClipSession>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn insert(session: ClipSession) {
    let play_id = session.play_id().to_string();
    registry()
        .lock()
        .expect("player registry lock")
        .insert(play_id, session);
}

pub fn get(play_id: &str) -> Option<ClipPlayerStatusSnapshot> {
    let guard = registry().lock().expect("player registry lock");
    guard.get(play_id).map(|s| ClipPlayerStatusSnapshot {
        status: s.status(),
    })
}

pub fn stop(play_id: &str) -> bool {
    let mut guard = registry().lock().expect("player registry lock");
    if let Some(session) = guard.get(play_id) {
        session.stop();
        true
    } else {
        false
    }
}

pub fn remove(play_id: &str) {
    let mut guard = registry().lock().expect("player registry lock");
    guard.remove(play_id);
}

/// Lightweight status snapshot for registry queries.
pub struct ClipPlayerStatusSnapshot {
    pub status: crate::types::ClipPlayerStatus,
}

pub fn start_from_path(play_id: String, path: &str) -> Result<String, PlayerError> {
    let session = ClipSession::start_from_path(play_id.clone(), path)?;
    insert(session);
    Ok(play_id)
}

pub fn start_from_bytes(play_id: String, data: Vec<u8>) -> Result<String, PlayerError> {
    let session = ClipSession::start_from_bytes(play_id.clone(), data)?;
    insert(session);
    Ok(play_id)
}

pub fn start_from_growing(play_id: String) -> Result<(String, crate::source::GrowingByteWriter), PlayerError> {
    let (session, writer) = ClipSession::start_from_growing(play_id.clone());
    insert(session);
    Ok((play_id, writer))
}
