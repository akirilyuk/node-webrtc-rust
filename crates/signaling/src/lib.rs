//! Optional Rust-side signaling server.
//!
//! Provides HTTP and WebSocket endpoints for SDP exchange and ICE candidate relay.

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
