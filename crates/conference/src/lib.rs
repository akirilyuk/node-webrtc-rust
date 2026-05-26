//! Conference room lifecycle, participant mixing, and signaling DTOs.
//!
//! Owns per-participant peer connections, RTP ingest into [`MixGraph`], and
//! personalized outbound [`LocalAudioTrack`] rendering.

mod error;
mod events;
mod mute;
mod participant;
mod room;
mod server;
mod signaling;

pub use error::{ConferenceError, ConferenceErrorCode};
pub use events::{
    MixingEnabledChanged, ParticipantJoined, ParticipantKicked, ParticipantLeft,
    ParticipantMuted, RoomError, RoomEvents,
};
pub use mute::{MuteMatrix, MuteScope};
pub use participant::Participant;
pub use room::{ParticipantInfo, Room, RoomConfig};
pub use server::ConferenceServer;
pub use signaling::{SignalingMessage, SignalingResponse};

/// Crate version string (matches `CARGO_PKG_VERSION`).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
