//! PCM utilities for voice activity detection.

use bytes::Bytes;

use crate::error::{SpeechError, SpeechResult};

/// Input PCM format from WebRTC remote tracks (48 kHz stereo interleaved i16).
pub const WEBRTC_PCM_SAMPLE_RATE: u32 = 48_000;
pub const WEBRTC_PCM_CHANNELS: usize = 2;

/// Mono PCM sample rate fed to STT vendors after downmix/downsample.
pub const STT_PCM_SAMPLE_RATE: u32 = 16_000;

/// Minimum mono PCM bytes before batch STT vendors attempt transcription (~100 ms @ 16 kHz i16).
pub const STT_MIN_BATCH_BYTES: usize = 3_200;

/// Mono 16 kHz s16le silence for a given duration (used for streaming STT endpoint tails).
pub fn silence_mono_s16le_bytes(duration_ms: u32) -> Bytes {
    let samples = (STT_PCM_SAMPLE_RATE as usize * duration_ms as usize / 1000).max(1);
    Bytes::from(vec![0u8; samples * 2])
}

/// Preferred batch size for utterance-style cloud STT (~1 s @ 16 kHz i16).
pub const STT_PREFERRED_BATCH_BYTES: usize = 32_000;

/// Convert stereo 48 kHz interleaved i16 PCM to mono 16 kHz i16 samples.
pub fn stereo_48k_to_mono_16k(pcm: &[u8]) -> Vec<i16> {
    if pcm.len() < 4 {
        return Vec::new();
    }

    let samples: Vec<i16> = pcm
        .as_chunks::<2>()
        .0
        .iter()
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
        let avg = (i32::from(input[i]) + i32::from(input[i + 1]) + i32::from(input[i + 2])) / 3;
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
    for sample in mono.as_chunks::<2>().0 {
        stereo.extend_from_slice(sample);
        stereo.extend_from_slice(sample);
    }
    Bytes::from(stereo)
}

/// Convert mono s16le PCM bytes to normalized f32 samples in [-1.0, 1.0].
pub fn mono_s16le_bytes_to_f32(pcm: &[u8]) -> Vec<f32> {
    pcm.as_chunks::<2>()
        .0
        .iter()
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

/// Streaming mono s16le at `src_rate` Hz to the agent's 48 kHz stereo s16le, with linear
/// interpolation (catalog `audio.sample-rate-mismatch`: vendor PCM must be converted, never
/// forwarded at its own rate).
///
/// Continuous across [`Self::push`] calls: the result of any split of the input equals the
/// result of pushing it whole, so chunk boundaries never click. An odd trailing byte of one call
/// is kept for the next. The output position is tracked in exact integer units (1/48000 of a
/// source sample), so there is no drift. [`Self::finish`] holds the last sample to flush the tail
/// (total output is `input samples x 48000 / src_rate`, to within one sample). 48000 Hz is a
/// passthrough that only duplicates the channel.
#[derive(Debug, Clone)]
pub struct MonoToStereo48k {
    src_rate: u32,
    /// Previous source sample (the left neighbour of the next output position).
    last: Option<i16>,
    /// Next output position past `last`, in 1/48000 of a source sample; always `< 48000`.
    pos: u32,
    odd: Option<u8>,
}

impl MonoToStereo48k {
    /// `src_rate` must be within 8000..=48000 Hz.
    pub fn new(src_rate: u32) -> SpeechResult<Self> {
        if !(8_000..=WEBRTC_PCM_SAMPLE_RATE).contains(&src_rate) {
            return Err(SpeechError::Config(format!(
                "unsupported TTS sample rate {src_rate} Hz; supported: 8000..=48000"
            )));
        }
        Ok(Self {
            src_rate,
            last: None,
            pos: 0,
            odd: None,
        })
    }

    /// Converts `mono_s16le` (an odd trailing byte is carried to the next call).
    pub fn push(&mut self, mono_s16le: &[u8]) -> Vec<u8> {
        let mut input = Vec::with_capacity(mono_s16le.len() + 1);
        if let Some(b) = self.odd.take() {
            input.push(b);
        }
        input.extend_from_slice(mono_s16le);
        if input.len() % 2 == 1 {
            self.odd = input.pop();
        }
        let mut out = Vec::with_capacity(input.len() * 2 * 48_000 / self.src_rate as usize + 8);
        for pair in input.as_chunks::<2>().0 {
            let cur = i16::from_le_bytes(*pair);
            if self.src_rate == WEBRTC_PCM_SAMPLE_RATE {
                push_frame(&mut out, cur);
                continue;
            }
            match self.last {
                None => {
                    self.last = Some(cur);
                    self.pos = 0;
                }
                Some(prev) => {
                    while self.pos < WEBRTC_PCM_SAMPLE_RATE {
                        push_frame(&mut out, lerp(prev, cur, self.pos));
                        self.pos += self.src_rate;
                    }
                    self.pos -= WEBRTC_PCM_SAMPLE_RATE;
                    self.last = Some(cur);
                }
            }
        }
        out
    }

    /// Flushes the tail (the last sample held until the end of its interval) and resets.
    pub fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(prev) = self.last.take() {
            while self.pos < WEBRTC_PCM_SAMPLE_RATE {
                push_frame(&mut out, prev);
                self.pos += self.src_rate;
            }
        }
        self.pos = 0;
        self.odd = None;
        out
    }
}

fn lerp(a: i16, b: i16, pos: u32) -> i16 {
    let (a, b) = (i64::from(a), i64::from(b));
    (a + (b - a) * i64::from(pos) / i64::from(WEBRTC_PCM_SAMPLE_RATE)) as i16
}

fn push_frame(out: &mut Vec<u8>, sample: i16) {
    let bytes = sample.to_le_bytes();
    out.extend_from_slice(&bytes);
    out.extend_from_slice(&bytes);
}

/// Duration in ms of 48 kHz stereo s16le bytes (192 bytes per ms), at least 1.
pub fn duration_ms_from_stereo_48k(byte_len: usize) -> u32 {
    ((byte_len / 192) as u32).max(1)
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

    fn mono_bytes(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn left_channel(stereo: &[u8]) -> Vec<i16> {
        stereo
            .as_chunks::<4>()
            .0
            .iter()
            .map(|f| i16::from_le_bytes([f[0], f[1]]))
            .collect()
    }

    fn convert_whole(rate: u32, input: &[u8]) -> Vec<u8> {
        let mut up = MonoToStereo48k::new(rate).unwrap();
        let mut out = up.push(input);
        out.extend(up.finish());
        out
    }

    #[test]
    fn upsampler_16k_ramp_is_monotonic_and_3x_long() {
        let ramp: Vec<i16> = (0..1600).map(|i| (i * 10 - 8000) as i16).collect();
        let out = convert_whole(16_000, &mono_bytes(&ramp));
        let left = left_channel(&out);
        assert_eq!(left.len(), 1600 * 3);
        assert!(left.windows(2).all(|w| w[0] <= w[1]), "not monotonic");
        assert!(left.windows(2).any(|w| w[0] < w[1]));
        // Both channels carry the same sample.
        assert!(out.as_chunks::<4>().0.iter().all(|f| f[0..2] == f[2..4]));
        // Interpolated, not held: neighbours between source samples differ.
        assert!(left[1] > left[0] && left[1] < left[3]);
    }

    #[test]
    fn upsampler_chunked_push_equals_one_shot_push() {
        let samples: Vec<i16> = (0..700)
            .map(|i| ((i as f64 * 0.37).sin() * 12_000.0) as i16)
            .collect();
        let input = mono_bytes(&samples);
        for rate in [8_000, 16_000, 22_050, 24_000, 44_100, 48_000] {
            let whole = convert_whole(rate, &input);
            for split in 1..=7usize {
                let mut up = MonoToStereo48k::new(rate).unwrap();
                let mut out = Vec::new();
                for part in input.chunks(split) {
                    out.extend(up.push(part));
                }
                out.extend(up.finish());
                assert_eq!(out, whole, "rate {rate}, split {split}");
            }
        }
    }

    #[test]
    fn upsampler_24k_sine_keeps_its_frequency() {
        // 440 Hz for 2 s at 24 kHz.
        let samples: Vec<i16> = (0..48_000)
            .map(|i| {
                ((i as f64 / 24_000.0 * 440.0 * std::f64::consts::TAU).sin() * 10_000.0) as i16
            })
            .collect();
        let left = left_channel(&convert_whole(24_000, &mono_bytes(&samples)));
        assert_eq!(left.len(), 96_000);
        let crossings = left.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count() as f64;
        let per_second = crossings / 2.0;
        assert!(
            (per_second - 880.0).abs() <= 8.8,
            "{per_second} crossings/s, expected 880 within 1%"
        );
    }

    #[test]
    fn upsampler_48k_is_passthrough() {
        let samples = [1i16, -2, 300, -400, 5];
        let out = convert_whole(48_000, &mono_bytes(&samples));
        let expected: Vec<u8> = samples
            .iter()
            .flat_map(|s| {
                let b = s.to_le_bytes();
                [b[0], b[1], b[0], b[1]]
            })
            .collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn upsampler_rate_outside_8k_to_48k_is_a_config_error() {
        for rate in [0, 7_999, 48_001, 96_000] {
            assert!(
                matches!(MonoToStereo48k::new(rate), Err(SpeechError::Config(_))),
                "{rate}"
            );
        }
        assert!(MonoToStereo48k::new(8_000).is_ok());
        assert!(MonoToStereo48k::new(48_000).is_ok());
    }

    #[test]
    fn stereo_48k_duration_is_192_bytes_per_ms() {
        assert_eq!(duration_ms_from_stereo_48k(192_000), 1000);
        assert_eq!(duration_ms_from_stereo_48k(0), 1);
    }
}
