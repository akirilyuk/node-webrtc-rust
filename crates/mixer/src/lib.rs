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
mod elastic;
mod frame;
mod graph;
mod otel;
pub mod pacer;
mod spatial;

pub use bus::MixBus;
pub use decode::{DecodeError, OpusDecoder};
pub use elastic::{ElasticFrameQueue, SourceStats};
pub use frame::{
    silence_frame, Frame, FrameBuffer, CHANNELS, FRAME_BYTES, FRAME_HOLD_MS, FRAME_MS,
    SAMPLES_PER_CHANNEL, SAMPLES_PER_FRAME, SAMPLE_RATE,
};
pub use graph::{MixGraph, ParticipantId};
pub use pacer::{DueSlots, OutboundPacer, MAX_BURST_WRITES};
pub use spatial::{
    apply_pan_gains, pan_frame_with_placement, pan_gains_placement, pan_gains_positional,
    ClientPose, DistanceParams, MixPlacement, PanGains, Quat, SpatialError, Vec3,
};

/// Crate version string (matches `CARGO_PKG_VERSION`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
