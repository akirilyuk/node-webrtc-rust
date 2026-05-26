//! Conference error types with stable codes for the bindings layer.

use thiserror::Error;

/// Stable error codes surfaced to TypeScript via NAPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConferenceErrorCode {
    RoomNotFound,
    ParticipantNotFound,
    RoomFull,
    InvalidMuteScope,
    SignalingError,
    Internal,
}

impl ConferenceErrorCode {
    /// Returns the wire-format code string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RoomNotFound => "ROOM_NOT_FOUND",
            Self::ParticipantNotFound => "PARTICIPANT_NOT_FOUND",
            Self::RoomFull => "ROOM_FULL",
            Self::InvalidMuteScope => "INVALID_MUTE_SCOPE",
            Self::SignalingError => "SIGNALING_ERROR",
            Self::Internal => "INTERNAL",
        }
    }
}

impl std::fmt::Display for ConferenceErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from conference room and server operations.
#[derive(Debug, Error)]
pub enum ConferenceError {
    #[error("{code}: {message}")]
    Coded {
        code: ConferenceErrorCode,
        message: String,
    },
    #[error("core error: {0}")]
    Core(#[from] node_webrtc_rust_core::CoreError),
}

impl ConferenceError {
    pub fn room_not_found(message: impl Into<String>) -> Self {
        Self::Coded {
            code: ConferenceErrorCode::RoomNotFound,
            message: message.into(),
        }
    }

    pub fn participant_not_found(message: impl Into<String>) -> Self {
        Self::Coded {
            code: ConferenceErrorCode::ParticipantNotFound,
            message: message.into(),
        }
    }

    pub fn room_full(message: impl Into<String>) -> Self {
        Self::Coded {
            code: ConferenceErrorCode::RoomFull,
            message: message.into(),
        }
    }

    pub fn invalid_mute_scope(message: impl Into<String>) -> Self {
        Self::Coded {
            code: ConferenceErrorCode::InvalidMuteScope,
            message: message.into(),
        }
    }

    pub fn signaling_error(message: impl Into<String>) -> Self {
        Self::Coded {
            code: ConferenceErrorCode::SignalingError,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Coded {
            code: ConferenceErrorCode::Internal,
            message: message.into(),
        }
    }

    pub fn code(&self) -> ConferenceErrorCode {
        match self {
            Self::Coded { code, .. } => *code,
            Self::Core(_) => ConferenceErrorCode::Internal,
        }
    }
}
