//! Inbound-audio preprocessors for the speaker/denoiser bench: the stages the voice agent could
//! run on the 20 ms 48 kHz stereo s16le frames before the VAD / STT split.
//!
//! - [`Preprocessor`]: `Raw` (no change), `Rnnoise` (the product's `Stereo48kRnnoise`) or
//!   `Sherpa` (GTCRN or DPDFNet through `OnlineSpeechDenoiser`). `process` returns exactly as
//!   many bytes as it was given.
//! - [`Agc`]: the caller-gain algorithm the product would copy (see its doc comment).
//!
//! Only these sherpa-onnx 1.13.2 APIs are used: `OnlineSpeechDenoiser::{create, run,
//! frame_shift_in_samples, sample_rate}`, `OnlineSpeechDenoiserConfig`,
//! `OfflineSpeechDenoiserModelConfig` and `DenoisedAudio { samples, sample_rate }`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use node_webrtc_rust_denoise::Stereo48kRnnoise;
use sherpa_onnx::{
    OfflineSpeechDenoiserModelConfig, OnlineSpeechDenoiser, OnlineSpeechDenoiserConfig,
};

/// Bytes in one 20 ms stereo 48 kHz s16le frame.
#[allow(dead_code)]
pub const FRAME_BYTES: usize = 3_840;

const SAMPLE_RATE_48K: i32 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SherpaKind {
    Gtcrn,
    Dpdfnet,
}

/// The state of one Sherpa denoiser stream: 48 kHz mono in, model rate inside, 48 kHz mono out.
pub struct SherpaState {
    denoiser: OnlineSpeechDenoiser,
    /// Model sample rate (16 000, or 48 000 for `dpdfnet2_48khz_hr`).
    model_sr: i32,
    /// 48 kHz samples per model-rate sample (3 or 1).
    ratio: usize,
    /// 48 kHz mono samples not yet averaged down to the model rate.
    in_rem: Vec<f32>,
    /// Last model-rate output sample, for the linear upsampler.
    prev_out: f32,
    /// Denoised 48 kHz mono samples not yet handed out. Pre-filled with silence equal to the
    /// model's frame shift so every call can return its input length.
    fifo: VecDeque<f32>,
    /// Calls that found the FIFO shorter than the input (the shortfall was padded with silence).
    underruns: usize,
}

pub enum Preprocessor {
    Raw,
    Rnnoise(Box<Stereo48kRnnoise>),
    Sherpa {
        kind: SherpaKind,
        model: PathBuf,
        state: Box<SherpaState>,
    },
}

impl Preprocessor {
    pub fn rnnoise() -> Self {
        Self::Rnnoise(Box::new(Stereo48kRnnoise::new()))
    }

    /// A Sherpa denoiser stream: exactly one of `gtcrn.model` / `dpdfnet.model`, one thread, CPU.
    pub fn sherpa(kind: SherpaKind, model: &Path) -> Result<Self, String> {
        let path = Some(model.to_string_lossy().into_owned());
        let mut model_config = OfflineSpeechDenoiserModelConfig {
            num_threads: 1,
            provider: Some("cpu".to_string()),
            ..Default::default()
        };
        match kind {
            SherpaKind::Gtcrn => model_config.gtcrn.model = path,
            SherpaKind::Dpdfnet => model_config.dpdfnet.model = path,
        }
        let denoiser = OnlineSpeechDenoiser::create(&OnlineSpeechDenoiserConfig {
            model: model_config,
        })
        .ok_or_else(|| format!("cannot open denoiser model {}", model.display()))?;
        let model_sr = denoiser.sample_rate();
        if model_sr <= 0 || SAMPLE_RATE_48K % model_sr != 0 {
            return Err(format!(
                "denoiser {} runs at {model_sr} Hz, which does not divide 48 kHz",
                model.display()
            ));
        }
        let ratio = (SAMPLE_RATE_48K / model_sr) as usize;
        let shift = denoiser.frame_shift_in_samples().max(0) as usize * ratio;
        Ok(Self::Sherpa {
            kind,
            model: model.to_path_buf(),
            state: Box::new(SherpaState {
                denoiser,
                model_sr,
                ratio,
                in_rem: Vec::new(),
                prev_out: 0.0,
                fifo: std::iter::repeat_n(0.0, shift).collect(),
                underruns: 0,
            }),
        })
    }

    /// Processes interleaved stereo 48 kHz s16le PCM; the output has the input's length.
    pub fn process(&mut self, frame_48k_stereo_s16: &[u8]) -> Vec<u8> {
        match self {
            Self::Raw => frame_48k_stereo_s16.to_vec(),
            Self::Rnnoise(rnnoise) => {
                let out = rnnoise.process_s16le_stereo(frame_48k_stereo_s16);
                debug_assert_eq!(out.len(), frame_48k_stereo_s16.len());
                out
            }
            Self::Sherpa { state, .. } => state.process(frame_48k_stereo_s16),
        }
    }

    /// `raw`, `rnnoise` or `<kind> <model file>`, for log lines.
    pub fn describe(&self) -> String {
        match self {
            Self::Raw => "raw".to_string(),
            Self::Rnnoise(_) => "rnnoise".to_string(),
            Self::Sherpa { kind, model, .. } => format!(
                "{kind:?} {}",
                model.file_name().and_then(|f| f.to_str()).unwrap_or("?")
            ),
        }
    }

    /// Calls whose output had to be padded with silence (always 0 for `Raw` and `Rnnoise`).
    pub fn underruns(&self) -> usize {
        match self {
            Self::Sherpa { state, .. } => state.underruns,
            _ => 0,
        }
    }
}

impl SherpaState {
    fn process(&mut self, pcm: &[u8]) -> Vec<u8> {
        let frames = pcm.len() / 4;
        // Mono downmix, then average down to the model rate (3 samples -> 1, like the product's
        // `stereo_48k_to_mono_16k`).
        let mono: Vec<f32> = pcm
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                let l = f32::from(i16::from_le_bytes([c[0], c[1]]));
                let r = f32::from(i16::from_le_bytes([c[2], c[3]]));
                (l + r) * 0.5 / 32_768.0
            })
            .collect();
        self.in_rem.extend_from_slice(&mono);
        let usable = self.in_rem.len() / self.ratio * self.ratio;
        let down: Vec<f32> = self.in_rem[..usable]
            .chunks_exact(self.ratio)
            .map(|c| c.iter().sum::<f32>() / self.ratio as f32)
            .collect();
        self.in_rem.drain(..usable);

        if !down.is_empty() {
            let out = self.denoiser.run(&down, self.model_sr);
            let up = if out.sample_rate > 0 && SAMPLE_RATE_48K % out.sample_rate == 0 {
                (SAMPLE_RATE_48K / out.sample_rate) as usize
            } else {
                self.ratio
            };
            for x in out.samples {
                for k in 1..=up {
                    self.fifo
                        .push_back(self.prev_out + (x - self.prev_out) * k as f32 / up as f32);
                }
                self.prev_out = x;
            }
        }

        if self.fifo.len() < frames {
            self.underruns += 1;
        }
        let mut out = Vec::with_capacity(pcm.len());
        for _ in 0..frames {
            let x = self.fifo.pop_front().unwrap_or(0.0);
            let s = (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
            let b = s.to_le_bytes();
            out.extend_from_slice(&b);
            out.extend_from_slice(&b);
        }
        // A trailing partial sample pair keeps the length contract.
        out.resize(pcm.len(), 0);
        out
    }
}

// ---------------------------------------------------------------------------------------------
// AGC
// ---------------------------------------------------------------------------------------------

/// Caller-gain AGC, the exact algorithm the product would copy. It runs per 20 ms 48 kHz stereo
/// frame, after the preprocessor and before the VAD / STT split:
///
/// - `frame_db` = 20 log10(RMS of the mono downmix, floor 1e-6).
/// - Noise floor `n` (start -60 dB): `n = min(n + 0.02, frame_db)` per frame (falls at once, rises
///   1 dB/s).
/// - A frame is speech when `frame_db > n + 10`.
/// - Speech level `s` (start -30 dB): on speech frames only, `s += 0.05 (frame_db - s)`.
/// - Target gain `t = clamp(-18 - s, 0, +18)` dB. Applied gain `g` (start 0) moves
///   `g += 0.02 (t - g)` per frame.
/// - The frame is multiplied by `10^(g/20)`. If its sample peak would exceed -1 dBFS (0.891), the
///   whole frame is scaled down to exactly that peak.
pub struct Agc {
    noise_db: f32,
    speech_db: f32,
    gain_db: f32,
}

impl Default for Agc {
    fn default() -> Self {
        Self::new()
    }
}

/// Sample peak ceiling of the limiter (-1 dBFS).
pub const AGC_PEAK: f32 = 0.891;

impl Agc {
    pub fn new() -> Self {
        Self {
            noise_db: -60.0,
            speech_db: -30.0,
            gain_db: 0.0,
        }
    }

    /// The gain applied to the latest frame, in dB.
    #[allow(dead_code)]
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    pub fn process(&mut self, frame_48k_stereo_s16: &[u8]) -> Vec<u8> {
        let samples: Vec<[f32; 2]> = frame_48k_stereo_s16
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| {
                [
                    f32::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0,
                    f32::from(i16::from_le_bytes([c[2], c[3]])) / 32_768.0,
                ]
            })
            .collect();
        if samples.is_empty() {
            return frame_48k_stereo_s16.to_vec();
        }
        let mean_sq = samples
            .iter()
            .map(|[l, r]| {
                let m = (l + r) * 0.5;
                f64::from(m * m)
            })
            .sum::<f64>()
            / samples.len() as f64;
        let frame_db = 20.0 * (mean_sq.sqrt() as f32).max(1e-6).log10();

        self.noise_db = (self.noise_db + 0.02).min(frame_db);
        if frame_db > self.noise_db + 10.0 {
            self.speech_db += 0.05 * (frame_db - self.speech_db);
        }
        let target = (-18.0 - self.speech_db).clamp(0.0, 18.0);
        self.gain_db += 0.02 * (target - self.gain_db);

        let mut gain = 10f32.powf(self.gain_db / 20.0);
        let peak = samples
            .iter()
            .flat_map(|[l, r]| [l.abs(), r.abs()])
            .fold(0.0f32, f32::max);
        if peak * gain > AGC_PEAK {
            gain = AGC_PEAK / peak;
        }
        let mut out = Vec::with_capacity(frame_48k_stereo_s16.len());
        for [l, r] in samples {
            for x in [l, r] {
                let s = (x * gain * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        out.resize(frame_48k_stereo_s16.len(), 0);
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers shared by the bench and the tests
// ---------------------------------------------------------------------------------------------

/// Runs `pcm` (48 kHz stereo s16le) through the preprocessor and the optional AGC in 20 ms
/// frames and returns the processed PCM (same length).
pub fn process_pcm(pcm: &[u8], pre: &mut Preprocessor, agc: Option<&mut Agc>) -> Vec<u8> {
    let mut agc = agc;
    let mut out = Vec::with_capacity(pcm.len());
    for frame in pcm.chunks(FRAME_BYTES) {
        let cleaned = pre.process(frame);
        match agc.as_deref_mut() {
            Some(agc) => out.extend_from_slice(&agc.process(&cleaned)),
            None => out.extend_from_slice(&cleaned),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stereo 48 kHz s16le tone of `ms` milliseconds with the given RMS in dBFS.
    fn tone(rms_db: f32, ms: usize, phase: &mut usize) -> Vec<u8> {
        let amp = 10f32.powf(rms_db / 20.0) * std::f32::consts::SQRT_2;
        let mut pcm = Vec::with_capacity(ms * 48 * 4);
        for _ in 0..ms * 48 {
            let t = *phase as f32 / 48_000.0;
            *phase += 1;
            let s =
                ((t * 300.0 * 2.0 * std::f32::consts::PI).sin() * amp * 32_768.0).round() as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        pcm
    }

    fn rms_db(frame: &[u8]) -> f32 {
        let n = frame.len() / 4;
        let sum: f64 = frame
            .chunks_exact(4)
            .map(|c| {
                let l = f64::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0;
                l * l
            })
            .sum();
        20.0 * ((sum / n as f64).sqrt() as f32).max(1e-6).log10()
    }

    #[test]
    fn agc_lifts_quiet_speech() {
        // 300 ms of -30 dBFS tone, 300 ms of -70 dBFS, for 5 s (20 ms frames: 15 + 15).
        let mut agc = Agc::new();
        let mut phase = 0;
        let mut speech_db_last_second = Vec::new();
        let frames_total = 250;
        for i in 0..frames_total {
            let speech = (i / 15) % 2 == 0;
            let frame = tone(if speech { -30.0 } else { -70.0 }, 20, &mut phase);
            let out = agc.process(&frame);
            if speech && i >= frames_total - 50 {
                speech_db_last_second.push(rms_db(&out));
            }
        }
        assert!(!speech_db_last_second.is_empty());
        for db in speech_db_last_second {
            assert!((db - -18.0).abs() <= 2.0, "speech frame at {db} dBFS");
        }
    }

    #[test]
    fn agc_never_attenuates() {
        // A loud input is never made quieter. The gain may rise for the first second (the speech
        // level estimate starts at -30 dB) and then decays back to unity.
        let mut agc = Agc::new();
        let mut phase = 0;
        let frames_total = 500; // 10 s
        let mut last_second = Vec::new();
        for i in 0..frames_total {
            let frame = tone(-10.0, 20, &mut phase);
            let out = agc.process(&frame);
            let delta = rms_db(&out) - rms_db(&frame);
            assert!(delta > -0.1, "frame {i} attenuated by {delta} dB");
            if i >= frames_total - 50 {
                last_second.push(delta);
            }
        }
        for delta in last_second {
            assert!(delta.abs() <= 0.1, "steady state gain {delta} dB");
        }
    }

    #[test]
    fn agc_limiter_caps_peak() {
        let mut agc = Agc::new();
        // Full-scale square wave, 100 Hz.
        let mut frame = Vec::new();
        for i in 0..960 {
            let s: i16 = if (i / 240) % 2 == 0 { 32_767 } else { -32_767 };
            frame.extend_from_slice(&s.to_le_bytes());
            frame.extend_from_slice(&s.to_le_bytes());
        }
        let ceiling = (AGC_PEAK * 32_768.0).round() as i32 + 1;
        for _ in 0..100 {
            let out = agc.process(&frame);
            let peak = out
                .chunks_exact(2)
                .map(|c| i32::from(i16::from_le_bytes([c[0], c[1]])).abs())
                .max()
                .unwrap();
            assert!(
                peak <= ceiling,
                "peak {peak} above the limiter ceiling {ceiling}"
            );
        }
    }

    /// Directory with the denoiser models (`SHERPA_DENOISE_DIR`); `None` skips model-gated tests.
    fn denoise_dir() -> Option<PathBuf> {
        match std::env::var("SHERPA_DENOISE_DIR") {
            Ok(dir) if Path::new(&dir).is_dir() => Some(PathBuf::from(dir)),
            _ => {
                eprintln!("skipped: SHERPA_DENOISE_DIR is not set to a directory");
                None
            }
        }
    }

    /// Pink-ish noise (sum of a few low-passed white noises) with a fixed seed, mono f32.
    fn pink_noise(len: usize, seed: u64) -> Vec<f32> {
        let mut state = seed | 1;
        let mut poles = [0.0f32; 4];
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let white = ((state >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0;
                let mut sum = 0.0;
                for (i, p) in poles.iter_mut().enumerate() {
                    let a = 0.9f32.powi(i as i32 + 1);
                    *p = a * *p + (1.0 - a) * white;
                    sum += *p;
                }
                sum
            })
            .collect()
    }

    fn mono_to_stereo_s16(mono: &[f32]) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(mono.len() * 4);
        for x in mono {
            let s = (x * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        pcm
    }

    fn sherpa_models() -> Vec<(SherpaKind, &'static str)> {
        vec![
            (SherpaKind::Gtcrn, "gtcrn_simple.onnx"),
            (SherpaKind::Dpdfnet, "dpdfnet2.onnx"),
            (SherpaKind::Dpdfnet, "dpdfnet2_48khz_hr.onnx"),
        ]
    }

    #[test]
    fn preprocessor_preserves_length() {
        // 500 mixed 10 / 20 ms frames, voiced-ish content.
        let mut sizes = Vec::new();
        for i in 0..500 {
            sizes.push(if i % 3 == 0 { 480 } else { 960 });
        }
        let noise = pink_noise(sizes.iter().sum::<usize>(), 7);
        let mut pre_list: Vec<(String, Preprocessor)> = vec![
            ("raw".into(), Preprocessor::Raw),
            ("rnnoise".into(), Preprocessor::rnnoise()),
        ];
        if let Some(dir) = denoise_dir() {
            for (kind, file) in sherpa_models() {
                let model = dir.join(file);
                assert!(
                    model.is_file(),
                    "model-gated test: {} missing",
                    model.display()
                );
                pre_list.push((
                    file.to_string(),
                    Preprocessor::sherpa(kind, &model).unwrap(),
                ));
            }
        }
        for (name, mut pre) in pre_list {
            let mut at = 0;
            for (i, n) in sizes.iter().enumerate() {
                let frame = mono_to_stereo_s16(&noise[at..at + n]);
                at += n;
                let out = pre.process(&frame);
                assert_eq!(out.len(), frame.len(), "{name} frame {i}");
            }
        }
    }

    #[test]
    fn gtcrn_raises_snr() {
        let Some(dir) = denoise_dir() else { return };
        let model = dir.join("gtcrn_simple.onnx");
        assert!(
            model.is_file(),
            "model-gated test: {} missing",
            model.display()
        );
        // A real clean voice clip (Sherpa's speaker test wav, `.models/speaker` next to
        // `.models/enhance`), at 48 kHz, mixed with pink noise at 5 dB SNR.
        let wav = dir.join("..").join("speaker").join("fangjun-sr-1.wav");
        if !wav.is_file() {
            eprintln!(
                "skipped: {} is missing (npm run download-speaker)",
                wav.display()
            );
            return;
        }
        let clean = crate::support::resample_linear(
            &crate::support::read_wav(&wav).unwrap(),
            crate::support::SAMPLE_RATE,
            48_000,
        );
        let len = clean.len();
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let noise = pink_noise(len, 11);
        let scale = rms(&clean) / rms(&noise) / 10f32.powf(5.0 / 20.0);
        let noisy: Vec<f32> = clean
            .iter()
            .zip(&noise)
            .map(|(c, n)| c + n * scale)
            .collect();

        let mut pre = Preprocessor::sherpa(SherpaKind::Gtcrn, &model).unwrap();
        let out_pcm = process_pcm(&mono_to_stereo_s16(&noisy), &mut pre, None);
        let out: Vec<f32> = out_pcm
            .chunks_exact(4)
            .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0)
            .collect();
        // SNR against the clean signal, after removing the processing delay (best lag within
        // 100 ms) and the best scalar gain (a denoiser may change the level).
        let snr = |sig: &[f32], reference: &[f32]| {
            let mut best = f32::NEG_INFINITY;
            for lag in (0..4_800).step_by(8) {
                let n = reference.len() - lag;
                let a = &reference[..n];
                let b = &sig[lag..lag + n];
                let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
                let nn: f32 = b.iter().map(|y| y * y).sum();
                if nn == 0.0 {
                    continue;
                }
                let g = dot / nn;
                let err: f32 = a.iter().zip(b).map(|(x, y)| (x - g * y).powi(2)).sum();
                let sig_e: f32 = a.iter().map(|x| x * x).sum();
                best = best.max(10.0 * (sig_e / err.max(1e-12)).log10());
            }
            best
        };
        let before = snr(&noisy, &clean);
        let after = snr(&out, &clean);
        assert!(
            after >= before + 3.0,
            "SNR before {before:.1} dB, after {after:.1} dB"
        );
    }
}
