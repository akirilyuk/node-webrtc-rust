//! PCM audio mixing for conference calls.
//!
//! # Frame format
//!
//! All PCM in this crate is **48 kHz, stereo, 16-bit signed LE**, in **20 ms**
//! frames ([`frame::FRAME_BYTES`] = 3 840 bytes). Payloads cross API boundaries as
//! [`bytes::Bytes`] to avoid copies in the hot path.
//!
//! Conference code owns RTP read loops; this crate is frame-in / frame-out only.

mod bus;
mod decode;
mod frame;
mod graph;

pub use bus::MixBus;
pub use decode::{DecodeError, OpusDecoder};
pub use frame::{
    silence_frame, Frame, FrameBuffer, CHANNELS, FRAME_BYTES, FRAME_MS, SAMPLE_RATE,
    SAMPLES_PER_CHANNEL, SAMPLES_PER_FRAME,
};
pub use graph::{MixGraph, ParticipantId};

/// Crate version string (matches `CARGO_PKG_VERSION`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
