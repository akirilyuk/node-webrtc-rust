//! PCM frame types and per-participant buffering.
//!
//! # Frame format
//!
//! Each [`Frame`] holds interleaved stereo PCM at 48 kHz, 16-bit signed little-endian:
//!
//! - Sample rate: 48 000 Hz ([`SAMPLE_RATE`])
//! - Channels: 2 (L, R interleaved) ([`CHANNELS`])
//! - Duration: 20 ms ([`FRAME_MS`])
//! - Size: 3 840 bytes ([`FRAME_BYTES`]) = 960 samples × 2 channels × 2 bytes

use bytes::Bytes;
use std::time::{Duration, Instant};

/// PCM sample rate in Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// Number of interleaved PCM channels (stereo).
pub const CHANNELS: u16 = 2;

/// Frame duration in milliseconds.
pub const FRAME_MS: u32 = 20;

/// Hold window for the latest pushed frame before [`FrameBuffer::current`] returns silence.
///
/// 2 × 20 ms — covers one missed inbound tick without holding mic energy forever.
/// Multi-listener safe: `current()` does not consume the slot.
pub const FRAME_HOLD_MS: u32 = 2 * FRAME_MS;

/// Byte length of one 20 ms stereo frame (960 samples × 2 channels × 2 bytes).
pub const FRAME_BYTES: usize = 3_840;

/// Samples per channel in one frame.
pub const SAMPLES_PER_CHANNEL: usize = (SAMPLE_RATE as usize * FRAME_MS as usize) / 1000;

/// Total interleaved i16 samples in one frame.
pub const SAMPLES_PER_FRAME: usize = SAMPLES_PER_CHANNEL * CHANNELS as usize;

/// One 20 ms stereo PCM frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Interleaved stereo PCM (L, R, L, R, …), little-endian i16.
    pub pcm: Bytes,
    /// Optional capture or render timestamp in microseconds.
    pub timestamp_us: Option<u64>,
}

impl Frame {
    /// Creates a frame from PCM bytes. Length must equal [`FRAME_BYTES`].
    pub fn new(pcm: Bytes, timestamp_us: Option<u64>) -> Self {
        debug_assert_eq!(pcm.len(), FRAME_BYTES);
        Self { pcm, timestamp_us }
    }

    /// Returns true when the buffer holds exactly one frame of PCM.
    pub fn is_valid(&self) -> bool {
        self.pcm.len() == FRAME_BYTES
    }
}

/// Returns a silent 20 ms stereo frame (all zero samples).
pub fn silence_frame() -> Frame {
    Frame {
        pcm: Bytes::from_static(&[0u8; FRAME_BYTES]),
        timestamp_us: None,
    }
}

/// Latest-frame slot for one participant; empty or expired slots render as silence.
#[derive(Debug, Clone)]
struct StoredFrame {
    frame: Frame,
    pushed_at: Instant,
}

/// Latest-frame slot for one participant; empty slots render as silence.
#[derive(Debug, Clone, Default)]
pub struct FrameBuffer {
    latest: Option<StoredFrame>,
}

impl FrameBuffer {
    /// Creates an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores the latest frame for this participant.
    pub fn push(&mut self, frame: Frame) {
        self.latest = Some(StoredFrame {
            frame,
            pushed_at: Instant::now(),
        });
    }

    /// Returns the latest frame when still within [`FRAME_HOLD_MS`], or silence when
    /// empty or expired. Does not consume the slot (safe for multiple mix listeners).
    pub fn current(&self) -> Frame {
        match &self.latest {
            Some(stored) if stored.pushed_at.elapsed() <= Duration::from_millis(FRAME_HOLD_MS as u64) => {
                stored.frame.clone()
            }
            _ => silence_frame(),
        }
    }

    /// Clears the stored frame.
    pub fn clear(&mut self) {
        self.latest = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::{thread::sleep, time::Duration};

    fn loud_frame(participant: u8) -> Frame {
        let mut pcm = vec![0u8; FRAME_BYTES];
        let sample = i16::from(participant).wrapping_mul(5_000);
        for i in 0..SAMPLES_PER_FRAME {
            pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    #[test]
    fn frame_constants_match_20ms_stereo_48khz() {
        assert_eq!(SAMPLES_PER_CHANNEL, 960);
        assert_eq!(SAMPLES_PER_FRAME, 1_920);
        assert_eq!(FRAME_BYTES, 3_840);
        assert_eq!(FRAME_HOLD_MS, 40);
    }

    #[test]
    fn silence_frame_is_zeroed_and_valid() {
        let frame = silence_frame();
        assert!(frame.is_valid());
        assert!(frame.pcm.iter().all(|&b| b == 0));
    }

    #[test]
    fn frame_buffer_returns_silence_when_empty() {
        let buffer = FrameBuffer::new();
        let frame = buffer.current();
        assert_eq!(frame, silence_frame());
    }

    #[test]
    fn frame_buffer_returns_pushed_frame_immediately() {
        let mut buffer = FrameBuffer::new();
        let loud = loud_frame(3);
        buffer.push(loud.clone());
        assert_eq!(buffer.current(), loud);
    }

    #[test]
    fn frame_buffer_expires_after_hold_window() {
        let mut buffer = FrameBuffer::new();
        buffer.push(loud_frame(1));
        sleep(Duration::from_millis(FRAME_HOLD_MS as u64 + 5));
        assert_eq!(buffer.current(), silence_frame());
    }

    #[test]
    fn frame_buffer_multiple_reads_within_hold_window() {
        let mut buffer = FrameBuffer::new();
        let loud = loud_frame(2);
        buffer.push(loud.clone());
        assert_eq!(buffer.current(), loud);
        assert_eq!(buffer.current(), loud);
    }

    #[test]
    fn frame_buffer_clear_drops_slot() {
        let mut buffer = FrameBuffer::new();
        buffer.push(loud_frame(4));
        buffer.clear();
        assert_eq!(buffer.current(), silence_frame());
    }
}
