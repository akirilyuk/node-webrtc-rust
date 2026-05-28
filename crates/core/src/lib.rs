//! WebRTC core engine wrapping webrtc-rs.
//!
//! Manages peer connections, tracks, RTP, DTLS, and ICE.

pub mod config;
pub mod data_channel;
pub mod debug;
pub mod error;
pub mod events;
pub mod media;
pub mod pcm_audio_track;
pub mod pcm_encoder;
pub mod peer_connection;
pub mod rtp_sender;

pub use config::*;
pub use data_channel::*;
pub use debug::{debug_event, debug_fn, is_debug_enabled, set_debug_enabled};
pub use error::CoreError;
pub use events::*;
pub use media::*;
pub use peer_connection::*;
pub use rtp_sender::RtpSender;

pub use bytes::Bytes;

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
