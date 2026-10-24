//! Shared inbound audio stage: RTP reordering, loss detection and Opus concealment helpers.
//!
//! Everything here is pure (injected clock, seeded RNG) so it can be unit-tested without
//! sleeping. Wiring into `RemoteTrack` lives elsewhere.

pub mod conceal;
pub mod impair;
pub mod reorder;
pub mod stage;
pub mod stats;

pub use conceal::{conceal_lost, MAX_CONCEALED_PACKETS};
pub use impair::{Impairment, ImpairmentConfig};
pub use reorder::{InboundReorderBuffer, StageOut};
pub use stage::InboundStage;
pub use stats::{InboundAudioStageStats, InboundAudioStatsSnapshot};
