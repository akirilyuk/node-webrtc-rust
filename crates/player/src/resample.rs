//! Resample arbitrary channel layouts to stereo 48 kHz s16le PCM.

use bytes::Bytes;
use node_webrtc_rust_mixer::{CHANNELS, SAMPLE_RATE};

/// Streaming converter to stereo 48 kHz interleaved i16.
pub struct Stereo48kResampler {
    src_rate: u32,
    src_channels: usize,
    /// Interleaved source samples as i16.
    src: Vec<i16>,
    next_out: u64,
}

impl Stereo48kResampler {
    pub fn new(src_rate: u32, src_channels: usize) -> Self {
        Self {
            src_rate: src_rate.max(1),
            src_channels: src_channels.max(1),
            src: Vec::new(),
            next_out: 0,
        }
    }

    /// Push interleaved i16 PCM at the configured source rate/channels.
    pub fn push_i16_interleaved(&mut self, samples: &[i16]) -> Bytes {
        if samples.is_empty() {
            return Bytes::new();
        }
        self.src.extend_from_slice(samples);
        self.emit_available(false)
    }

    /// Flush remaining samples at end of stream.
    pub fn finish(&mut self) -> Bytes {
        self.emit_available(true)
    }

    fn emit_available(&mut self, finished: bool) -> Bytes {
        let frames = self.src.len() / self.src_channels;
        if frames == 0 {
            return Bytes::new();
        }

        let mono = if self.src_channels == 1 {
            self.emit_mono_passthrough(frames, finished)
        } else {
            self.emit_downmix_stereo(frames, finished)
        };

        if mono.is_empty() {
            return Bytes::new();
        }

        let stereo: Vec<u8> = mono
            .iter()
            .flat_map(|&sample| {
                let bytes = sample.to_le_bytes();
                [bytes[0], bytes[1], bytes[0], bytes[1]]
            })
            .collect();
        Bytes::from(stereo)
    }

    fn emit_mono_passthrough(&mut self, frames: usize, finished: bool) -> Vec<i16> {
        let dst_rate = SAMPLE_RATE;
        if self.src_rate == dst_rate {
            let already = self.next_out as usize;
            if already >= frames {
                return Vec::new();
            }
            let slice = &self.src[already..frames];
            self.next_out = frames as u64;
            return slice.to_vec();
        }
        self.emit_linear_resample_mono(frames, dst_rate, finished)
    }

    fn emit_downmix_stereo(&mut self, frames: usize, finished: bool) -> Vec<i16> {
        let mut mono = Vec::with_capacity(frames);
        for frame_idx in 0..frames {
            let base = frame_idx * self.src_channels;
            let sum: i32 = self.src[base..base + self.src_channels]
                .iter()
                .map(|&s| i32::from(s))
                .sum();
            mono.push((sum / self.src_channels as i32) as i16);
        }
        self.src.clear();
        self.next_out = 0;
        let channels = self.src_channels;
        self.src_channels = 1;
        self.src.extend(mono);
        let out = self.emit_mono_passthrough(self.src.len(), finished);
        self.src_channels = channels;
        out
    }

    fn emit_linear_resample_mono(&mut self, frames: usize, dst_rate: u32, finished: bool) -> Vec<i16> {
        let src_rate = self.src_rate;
        let mut out = Vec::new();

        let max_out = if finished {
            ((frames as u64 * dst_rate as u64) / src_rate as u64) as usize + 1
        } else {
            let last_src = frames.saturating_sub(1) as u64;
            let max_out_idx = (last_src * dst_rate as u64) / src_rate as u64;
            if max_out_idx < self.next_out {
                return Vec::new();
            }
            (max_out_idx - self.next_out + 1) as usize
        };

        for _ in 0..max_out {
            let out_idx = self.next_out;
            let src_pos = (out_idx as f64 * src_rate as f64) / dst_rate as f64;
            let idx0 = src_pos.floor() as usize;
            if idx0 >= frames {
                break;
            }
            let idx1 = (idx0 + 1).min(frames - 1);
            let frac = (src_pos - idx0 as f64) as f32;
            let s0 = self.src[idx0] as f32;
            let s1 = self.src[idx1] as f32;
            let sample = s0 + (s1 - s0) * frac;
            out.push(sample.clamp(i16::MIN as f32, i16::MAX as f32) as i16);
            self.next_out += 1;
        }

        if finished {
            self.src.clear();
            self.next_out = 0;
        }

        out
    }
}

/// Convert f32 interleaved samples (any channels) to stereo 48k i16 bytes.
pub fn f32_interleaved_to_stereo_48k(
    samples: &[f32],
    src_rate: u32,
    src_channels: usize,
) -> Bytes {
    if samples.is_empty() || src_channels == 0 {
        return Bytes::new();
    }

    if src_rate == SAMPLE_RATE && src_channels == CHANNELS as usize {
        let mut stereo = Vec::with_capacity(samples.len() * 2);
        for &sample in samples {
            let v = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            stereo.extend_from_slice(&v.to_le_bytes());
        }
        return Bytes::from(stereo);
    }
    let frames = samples.len() / src_channels;
    let mut i16_samples = Vec::with_capacity(frames * src_channels);
    for frame in 0..frames {
        for ch in 0..src_channels {
            let v = samples[frame * src_channels + ch].clamp(-1.0, 1.0);
            i16_samples.push((v * i16::MAX as f32) as i16);
        }
    }
    let mut resampler = Stereo48kResampler::new(src_rate, src_channels);
    let mut out = resampler.push_i16_interleaved(&i16_samples).to_vec();
    out.extend_from_slice(&resampler.finish());
    Bytes::from(out)
}

pub fn stereo_byte_len_to_ms(byte_len: usize) -> u64 {
    if byte_len == 0 {
        return 0;
    }
    let samples_per_ch = byte_len / (2 * CHANNELS as usize);
    (samples_per_ch as u64 * 1000) / SAMPLE_RATE as u64
}
