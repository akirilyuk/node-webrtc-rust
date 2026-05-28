//! PCM utilities for voice activity detection.

use bytes::Bytes;

/// Input PCM format from WebRTC remote tracks (48 kHz stereo interleaved i16).
pub const WEBRTC_PCM_SAMPLE_RATE: u32 = 48_000;
pub const WEBRTC_PCM_CHANNELS: usize = 2;

/// Mono PCM sample rate fed to STT vendors after downmix/downsample.
pub const STT_PCM_SAMPLE_RATE: u32 = 16_000;

/// Minimum mono PCM bytes before batch STT vendors attempt transcription (~100 ms @ 16 kHz i16).
pub const STT_MIN_BATCH_BYTES: usize = 3_200;

/// Preferred batch size for utterance-style cloud STT (~1 s @ 16 kHz i16).
pub const STT_PREFERRED_BATCH_BYTES: usize = 32_000;

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

/// Duration in milliseconds for mono s16le PCM at the given sample rate.
pub fn duration_ms_from_mono_s16le(byte_len: usize, sample_rate: u32) -> u32 {
    if sample_rate == 0 {
        return 1;
    }
    let samples = byte_len / 2;
    ((samples as u64 * 1000) / sample_rate as u64).max(1) as u32
}

/// Duplicate mono s16le samples into stereo interleaved PCM (WebRTC outbound format).
pub fn mono_s16le_to_stereo(mono: &[u8]) -> Bytes {
    let mut stereo = Vec::with_capacity(mono.len() * 2);
    for sample in mono.chunks_exact(2) {
        stereo.extend_from_slice(sample);
        stereo.extend_from_slice(sample);
    }
    Bytes::from(stereo)
}

/// Convert mono s16le PCM bytes to normalized f32 samples in [-1.0, 1.0].
pub fn mono_s16le_bytes_to_f32(pcm: &[u8]) -> Vec<f32> {
    pcm.chunks_exact(2)
        .map(|chunk| {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            f32::from(sample) / 32768.0
        })
        .collect()
}

/// Convert mono i16 samples to little-endian bytes.
pub fn i16_samples_to_bytes(samples: &[i16]) -> Bytes {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Bytes::from(bytes)
}

/// Wrap mono 16-bit little-endian PCM (16 kHz) in a minimal WAV container for upload APIs.
pub fn mono16_le_to_wav(pcm_le: &[u8]) -> Vec<u8> {
    let data_size = pcm_le.len() as u32;
    let riff_size = 36 + data_size;
    let mut wav = Vec::with_capacity(44 + pcm_le.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&STT_PCM_SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(STT_PCM_SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm_le);
    wav
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
    fn mono16_wav_has_header_and_payload() {
        let pcm = vec![0_u8; 320];
        let wav = mono16_le_to_wav(&pcm);
        assert!(wav.starts_with(b"RIFF"));
        assert!(wav.len() >= 44 + pcm.len());
    }

    #[test]
    fn mono_s16le_bytes_to_f32_normalizes() {
        let pcm = i16::MAX.to_le_bytes();
        let samples = mono_s16le_bytes_to_f32(&pcm);
        assert_eq!(samples.len(), 1);
        assert!((samples[0] - (f32::from(i16::MAX) / 32768.0)).abs() < 1e-6);
    }
}
