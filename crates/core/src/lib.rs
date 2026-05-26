//! WebRTC core engine wrapping webrtc-rs.
//!
//! Manages peer connections, tracks, RTP, DTLS, and ICE.

pub mod config;
pub mod data_channel;
pub mod error;
pub mod events;
pub mod media;
pub mod peer_connection;

pub use config::*;
pub use data_channel::*;
pub use error::CoreError;
pub use events::*;
pub use media::*;
pub use peer_connection::*;

pub use bytes::Bytes;

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
