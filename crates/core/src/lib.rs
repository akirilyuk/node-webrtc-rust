//! WebRTC core engine wrapping webrtc-rs.
//!
//! Manages peer connections, tracks, RTP, DTLS, and ICE.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
