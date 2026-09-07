/// Lifecycle state of a clip play session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipStatus {
    Buffering,
    Playing,
    Stopped,
    Ended,
    Error,
}

/// Observable status for a clip play session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipPlayerStatus {
    pub play_id: String,
    pub status: ClipStatus,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub buffered_ms: u64,
    pub error: Option<String>,
}

impl ClipPlayerStatus {
    pub fn buffering(play_id: String) -> Self {
        Self {
            play_id,
            status: ClipStatus::Buffering,
            position_ms: 0,
            duration_ms: None,
            buffered_ms: 0,
            error: None,
        }
    }
}

/// Preroll window before transitioning from buffering to playing.
pub const PREROLL_MIN_MS: u64 = 200;
pub const PREROLL_TARGET_MS: u64 = 300;
pub const PREROLL_MAX_MS: u64 = 400;
