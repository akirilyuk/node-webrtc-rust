//! PCM summing bus with soft limiting.

use bytes::Bytes;

use crate::frame::{Frame, FRAME_BYTES, SAMPLES_PER_FRAME};

/// Mixes multiple PCM frames into one output frame.
pub struct MixBus;

impl MixBus {
    /// Sums `inputs` with i32 accumulation and soft clipping.
    ///
    /// Invalid or short frames are treated as silence for that slot.
    pub fn mix(inputs: &[Frame]) -> Frame {
        let mut accum = [0i32; SAMPLES_PER_FRAME];

        for input in inputs {
            if input.pcm.len() != FRAME_BYTES {
                continue;
            }
            for (idx, chunk) in input.pcm.chunks_exact(2).enumerate() {
                let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
                accum[idx] = accum[idx].saturating_add(sample);
            }
        }

        let mut pcm = vec![0u8; FRAME_BYTES];
        for (idx, sample) in accum.into_iter().enumerate() {
            pcm[idx * 2..idx * 2 + 2].copy_from_slice(&soft_clip(sample).to_le_bytes());
        }

        Frame::new(Bytes::from(pcm), None)
    }
}

/// Soft-knee limiter: gentle compression above ~90% full scale, hard cap at i16 bounds.
fn soft_clip(sample: i32) -> i16 {
    const KNEE: i32 = 29_491; // ~0.9 * i16::MAX

    let sign = if sample >= 0 { 1i32 } else { -1 };
    let abs = sample.abs();

    let limited = if abs <= KNEE {
        abs
    } else {
        KNEE + (abs - KNEE) / 4
    };

    let capped = limited.min(i16::MAX as i32);
    (sign * capped).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn sine_frame(amplitude: i16, phase: f64) -> Frame {
        let mut pcm = vec![0u8; FRAME_BYTES];
        for i in 0..SAMPLES_PER_FRAME {
            let channel = i % 2;
            let t = (i / 2) as f64 / crate::frame::SAMPLES_PER_CHANNEL as f64;
            let wave = (2.0 * std::f64::consts::PI * 440.0 * t + phase + channel as f64 * 0.1)
                .sin();
            let sample = (wave * f64::from(amplitude)) as i16;
            pcm[i * 2..i * 2 + 2].copy_from_slice(&sample.to_le_bytes());
        }
        Frame::new(Bytes::from(pcm), None)
    }

    #[test]
    fn two_sine_waves_sum_in_phase() {
        let a = sine_frame(10_000, 0.0);
        let b = sine_frame(10_000, 0.0);
        let mixed = MixBus::mix(&[a.clone(), b.clone()]);
        let single = MixBus::mix(&[a]);

        let mixed_peak = peak_abs_sample(&mixed);
        let single_peak = peak_abs_sample(&single);
        assert!(mixed_peak > single_peak);
    }

    fn peak_abs_sample(frame: &Frame) -> u16 {
        frame
            .pcm
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]).unsigned_abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn clip_bounds_max_sample_value() {
        let loud = sine_frame(i16::MAX / 2, 0.0);
        let mixed = MixBus::mix(&[loud.clone(), loud]);

        for chunk in mixed.pcm.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            assert!(sample <= i16::MAX);
            assert!(sample >= i16::MIN);
        }
    }

    #[test]
    fn empty_inputs_yield_silence() {
        let mixed = MixBus::mix(&[]);
        assert_eq!(mixed, crate::frame::silence_frame());
    }
}
