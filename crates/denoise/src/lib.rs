//! Stereo 48 kHz RNNoise preprocessor via [`nnnoiseless`].
//!
//! Operates on interleaved s16le stereo PCM in 20 ms frames (3 840 bytes @ 48 kHz).

use nnnoiseless::DenoiseState;

/// RNNoise frame size: 480 mono samples = 10 ms @ 48 kHz.
pub const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE;

/// Bytes in one 20 ms stereo 48 kHz s16le frame (960 samples per channel).
pub const STEREO_20MS_BYTES: usize = 3_840;

/// Half-frame stereo bytes (10 ms).
pub const STEREO_10MS_BYTES: usize = STEREO_20MS_BYTES / 2;

/// Inbound stereo PCM denoiser (compile-always; [`is_compiled`] is always true).
pub struct Stereo48kRnnoise {
    denoise: Box<DenoiseState<'static>>,
    remainder: Vec<f32>,
    out_buf: [f32; FRAME_SIZE],
}

impl Stereo48kRnnoise {
    /// Creates a denoiser primed with one silent RNNoise frame (streaming-safe; no output discard).
    pub fn new() -> Self {
        let mut denoise = DenoiseState::new();
        let silence = [0.0f32; FRAME_SIZE];
        let mut out_buf = [0.0f32; FRAME_SIZE];
        denoise.process_frame(&mut out_buf, &silence);

        Self {
            denoise,
            remainder: Vec::new(),
            out_buf: [0.0f32; FRAME_SIZE],
        }
    }

    /// Returns true — RNNoise is always compiled in this crate.
    pub fn is_compiled() -> bool {
        true
    }

    /// Denoises interleaved stereo s16le 48 kHz PCM; output length matches input length.
    pub fn process_s16le_stereo(&mut self, pcm: &[u8]) -> Vec<u8> {
        if pcm.is_empty() {
            return Vec::new();
        }

        let mono_in = downmix_stereo_s16le_to_mono_f32(pcm);
        self.remainder.extend_from_slice(&mono_in);

        let mut mono_out = Vec::with_capacity(mono_in.len());
        while self.remainder.len() >= FRAME_SIZE {
            let chunk: Vec<f32> = self.remainder.drain(..FRAME_SIZE).collect();
            self.denoise.process_frame(&mut self.out_buf, &chunk);
            mono_out.extend_from_slice(&self.out_buf);
        }

        stereo_s16le_from_mono_f32(&mono_out)
    }
}

impl Default for Stereo48kRnnoise {
    fn default() -> Self {
        Self::new()
    }
}

fn downmix_stereo_s16le_to_mono_f32(pcm: &[u8]) -> Vec<f32> {
    let sample_pairs = pcm.len() / 4;
    let mut mono = Vec::with_capacity(sample_pairs);
    for i in 0..sample_pairs {
        let off = i * 4;
        let left = i16::from_le_bytes([pcm[off], pcm[off + 1]]) as f32;
        let right = i16::from_le_bytes([pcm[off + 2], pcm[off + 3]]) as f32;
        mono.push((left + right) * 0.5);
    }
    mono
}

fn stereo_s16le_from_mono_f32(mono: &[f32]) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(mono.len() * 4);
    for &sample in mono {
        let clamped = sample.clamp(-32768.0, 32767.0) as i16;
        let bytes = clamped.to_le_bytes();
        pcm.extend_from_slice(&bytes);
        pcm.extend_from_slice(&bytes);
    }
    pcm
}

/// RMS of interleaved stereo s16le PCM in [0.0, 1.0] (per-channel average energy).
pub fn stereo_pcm_rms(pcm: &[u8]) -> f32 {
    if pcm.len() < 4 {
        return 0.0;
    }
    let mut sum_sq = 0.0f64;
    let mut count = 0usize;
    for chunk in pcm.chunks_exact(4) {
        for ch in [0, 2] {
            let s = i16::from_le_bytes([chunk[ch], chunk[ch + 1]]) as f64 / f64::from(i16::MAX);
            sum_sq += s * s;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    (sum_sq / count as f64).sqrt() as f32
}

fn stereo_silence_20ms() -> Vec<u8> {
    vec![0u8; STEREO_20MS_BYTES]
}

fn stereo_white_noise_20ms(seed: u32) -> Vec<u8> {
    let mut state = seed.max(1);
    let mut pcm = Vec::with_capacity(STEREO_20MS_BYTES);
    for _ in 0..960 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sample = ((state >> 16) as i16).wrapping_mul(4);
        pcm.extend_from_slice(&sample.to_le_bytes());
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

fn stereo_sine_440_20ms() -> Vec<u8> {
    let mut pcm = Vec::with_capacity(STEREO_20MS_BYTES);
    for i in 0..960 {
        let t = i as f32 / 48_000.0;
        let sample = (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * (i16::MAX as f32 * 0.35);
        let s = sample as i16;
        pcm.extend_from_slice(&s.to_le_bytes());
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    pcm
}

fn warmup_frames(denoiser: &mut Stereo48kRnnoise, n: usize) {
    let frame = stereo_white_noise_20ms(42);
    for i in 0..n {
        let mut f = frame.clone();
        f[0] ^= (i as u8).wrapping_mul(3);
        let _ = denoiser.process_s16le_stereo(&f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn twenty_ms_frame_length_preserved_across_warmup_and_many_frames() {
        let mut denoiser = Stereo48kRnnoise::new();
        warmup_frames(&mut denoiser, 3);

        for i in 0..20 {
            let input = stereo_white_noise_20ms(100 + i as u32);
            assert_eq!(input.len(), STEREO_20MS_BYTES);
            let output = denoiser.process_s16le_stereo(&input);
            assert_eq!(
                output.len(),
                input.len(),
                "frame {i}: output length must match input"
            );
        }
    }

    #[test]
    fn silence_rms_stays_low() {
        let mut denoiser = Stereo48kRnnoise::new();
        for _ in 0..5 {
            let _ = denoiser.process_s16le_stereo(&stereo_silence_20ms());
        }
        let output = denoiser.process_s16le_stereo(&stereo_silence_20ms());
        assert!(stereo_pcm_rms(&output) < 0.05);
    }

    #[test]
    fn white_noise_rms_drops_after_warmup() {
        let mut denoiser = Stereo48kRnnoise::new();
        warmup_frames(&mut denoiser, 5);

        let input = stereo_white_noise_20ms(7_777);
        let input_rms = stereo_pcm_rms(&input);
        let output = denoiser.process_s16le_stereo(&input);
        let output_rms = stereo_pcm_rms(&output);
        assert!(input_rms > 0.05, "input should be loud noise");
        assert!(
            output_rms < input_rms,
            "denoised RMS {output_rms} must be strictly less than input {input_rms}"
        );
    }

    #[test]
    fn sine_440hz_retains_energy_after_warmup() {
        let mut denoiser = Stereo48kRnnoise::new();
        warmup_frames(&mut denoiser, 5);

        let input = stereo_sine_440_20ms();
        let input_rms = stereo_pcm_rms(&input);
        let output = denoiser.process_s16le_stereo(&input);
        let output_rms = stereo_pcm_rms(&output);
        assert!(input_rms > 0.1);
        assert!(
            output_rms > input_rms * 0.2,
            "speech-like tone should retain energy: out={output_rms} in={input_rms}"
        );
    }

    #[test]
    fn split_ten_ms_equals_one_twenty_ms_frame() {
        let input = stereo_white_noise_20ms(99);
        let (first, second) = input.split_at(STEREO_10MS_BYTES);

        let mut whole = Stereo48kRnnoise::new();
        warmup_frames(&mut whole, 5);
        let whole_out = whole.process_s16le_stereo(&input);

        let mut split = Stereo48kRnnoise::new();
        warmup_frames(&mut split, 5);
        let mut split_out = split.process_s16le_stereo(first);
        split_out.extend_from_slice(&split.process_s16le_stereo(second));

        assert_eq!(whole_out.len(), split_out.len());
        assert_eq!(whole_out, split_out);
    }

    #[test]
    fn realtime_factor_under_budget() {
        let mut denoiser = Stereo48kRnnoise::new();
        let frame = stereo_white_noise_20ms(1);
        let start = Instant::now();
        for i in 0..50 {
            let mut f = frame.clone();
            f[2] ^= (i as u8).wrapping_mul(5);
            let _ = denoiser.process_s16le_stereo(&f);
        }
        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        let audio_s = 1.0;
        let rtf = wall_ms / 1000.0 / audio_s;
        eprintln!("[rnnoise-rtf] wall_ms={wall_ms:.3} rtf={rtf:.4}");
        assert!(
            rtf < 0.5,
            "RTF {rtf:.4} must be < 0.5 (wall_ms={wall_ms:.3})"
        );
    }

    #[test]
    fn is_compiled_always_true() {
        assert!(Stereo48kRnnoise::is_compiled());
    }
}
