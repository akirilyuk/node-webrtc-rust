//! PCM utilities for voice activity detection.

use bytes::Bytes;

/// Input PCM format from WebRTC remote tracks (48 kHz stereo interleaved i16).
pub const WEBRTC_PCM_SAMPLE_RATE: u32 = 48_000;
pub const WEBRTC_PCM_CHANNELS: usize = 2;

/// Convert stereo 48 kHz interleaved i16 PCM to mono 16 kHz i16 samples.
pub fn stereo_48k_to_mono_16k(pcm: &[u8]) -> Vec<i16> {
    if pcm.len() < 4 {
        return Vec::new();
    }

    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    let mut mono = Vec::with_capacity(samples.len() / WEBRTC_PCM_CHANNELS);
    for frame in samples.chunks(WEBRTC_PCM_CHANNELS) {
        let sum: i32 = frame.iter().map(|&s| i32::from(s)).sum();
        mono.push((sum / frame.len() as i32) as i16);
    }

    downsample_by_3(&mono)
}

fn downsample_by_3(input: &[i16]) -> Vec<i16> {
    let mut output = Vec::with_capacity(input.len() / 3 + 1);
    let mut i = 0;
    while i + 2 < input.len() {
        let avg = (i32::from(input[i])
            + i32::from(input[i + 1])
            + i32::from(input[i + 2]))
            / 3;
        output.push(avg as i16);
        i += 3;
    }
    output
}

/// RMS energy of mono i16 PCM in range [0.0, 1.0].
pub fn pcm_rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let norm = f64::from(s) / f64::from(i16::MAX);
            norm * norm
        })
        .sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Convert mono i16 samples to little-endian bytes.
pub fn i16_samples_to_bytes(samples: &[i16]) -> Bytes {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Bytes::from(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_reduces_length() {
        let mut stereo = Vec::new();
        for i in 0..480 {
            stereo.extend_from_slice(&(i as i16).to_le_bytes());
            stereo.extend_from_slice(&(i as i16).to_le_bytes());
        }
        let mono = stereo_48k_to_mono_16k(&stereo);
        assert!(!mono.is_empty());
        assert!(mono.len() < 480);
    }

    #[test]
    fn silence_has_low_rms() {
        let silence = vec![0_i16; 160];
        assert!(pcm_rms_i16(&silence) < 0.01);
    }

    #[test]
    fn loud_signal_has_higher_rms() {
        let loud = vec![i16::MAX / 4; 160];
        assert!(pcm_rms_i16(&loud) > 0.05);
    }
}
