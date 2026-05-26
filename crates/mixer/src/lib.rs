//! Audio and video mixing pipeline.
//!
//! Handles PCM audio summing and video compositing (decode, layout, encode).

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
