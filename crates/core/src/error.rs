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
