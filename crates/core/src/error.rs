//! Core error types for the WebRTC engine.

use thiserror::Error;

/// Errors returned by the core WebRTC engine.
#[derive(Error, Debug)]
pub enum CoreError {
    /// A webrtc-rs operation failed.
    #[error("WebRTC error: {0}")]
    WebRtc(#[from] webrtc::Error),

    /// An ICE-related error.
    #[error("ICE error: {0}")]
    Ice(String),

    /// A DataChannel-related error.
    #[error("DataChannel error: {0}")]
    DataChannel(String),

    /// A media track-related error.
    #[error("Track error: {0}")]
    Track(String),

    /// The peer connection is in an invalid state for the requested operation.
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Configuration is invalid or incomplete.
    #[error("Configuration error: {0}")]
    Config(String),
}

/// Returns true when a webrtc-rs error is expected during ICE/PC/data-channel teardown.
pub fn is_benign_teardown_error(err: &webrtc::Error) -> bool {
    is_benign_teardown_message(&err.to_string())
}

/// String matcher for teardown races (also used in unit tests).
pub fn is_benign_teardown_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("non-established state")
        || lower.contains("sending reset packet")
        || lower.contains("data channel is closed")
        || lower.contains("data_channel is closed")
        || lower.contains("connection is closed")
        || lower.contains("peerconnection is closed")
}

#[cfg(test)]
mod tests {
    use super::is_benign_teardown_message;

    #[test]
    fn recognizes_data_channel_reset_during_teardown() {
        assert!(is_benign_teardown_message(
            "data_channels: sending reset packet in non-Established state"
        ));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert!(!is_benign_teardown_message("DTLS handshake failed"));
    }
}
