//! Clip decoder and progressive playback for node-webrtc-rust.
//!
//! Decodes WAV, MP3, OGG, FLAC, AAC, M4A/MP4, and raw s16le 48 kHz stereo PCM into
//! 20 ms stereo frames compatible with [`node_webrtc_rust_mixer`].

mod decoder;
mod error;
mod mp4_probe;
mod player;
mod registry;
mod resample;
mod session;
mod source;
mod types;

pub use decoder::{hint_from_bytes, hint_from_path};
pub use error::PlayerError;
pub use mp4_probe::{mp4_ready_for_decode, probe_mp4_layout, Mp4Layout};
pub use player::{
    get_clip_status, play_clip_from_bytes, play_clip_from_path, play_clip_progressive, stop_clip,
};
pub use session::{split_frames, ClipSession};
pub use source::{GrowingByteSource, GrowingByteWriter, is_raw_pcm_s16le_48k_stereo};
pub use types::{ClipPlayerStatus, ClipStatus, PREROLL_MAX_MS, PREROLL_MIN_MS, PREROLL_TARGET_MS};

/// Crate version string (matches `CARGO_PKG_VERSION`).
pub fn version() -> &'static str {
    player::version()
}
