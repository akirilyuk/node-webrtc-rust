//! Whisper language scores for spoken language identification.
//!
//! sherpa-onnx `SpokenLanguageIdentification` returns only the argmax language code. To decide
//! among a project's allowed languages we need the per-language scores, so this module runs the
//! same Whisper encoder/decoder ONNX files sherpa loads and reads the decoder logits at the
//! language position after `<|startoftranscript|>`, as sherpa-onnx v1.13.2
//! `spoken-language-identification-whisper-impl.h` does.
//!
//! - Model I/O: positional, as in sherpa-onnx `scripts/whisper/test.py` (encoder: mel →
//!   cross_k, cross_v; decoder: tokens, self_k, self_v, cross_k, cross_v, offset → logits).
//! - Language tokens, codes and `sot` come from the encoder's own metadata
//!   (`all_language_tokens`, `all_language_codes`, `sot`, `n_mels`, `n_text_*`).
//! - Features: Whisper mel power spectrum as kaldi-native-fbank `WhisperFeatureComputer`
//!   (25 ms / 10 ms frames, periodic Hann, `snip_edges=false` reflection framing, 400-point power
//!   spectrum, librosa slaney mel), then `log10(max(x, 1e-10))`, clamp to `max - 8`,
//!   `(x + 4) / 4`, and 1000 zero frames of tail padding.

use std::sync::Mutex;

use node_webrtc_rust_speech::error::{SpeechError, SpeechResult};
use ort::session::Session;
use ort::value::Tensor;

use crate::lid_model_paths::LidModelPaths;

const SAMPLE_RATE: f64 = 16_000.0;
const FRAME_LENGTH: usize = 400;
const FRAME_SHIFT: usize = 160;
const N_FFT_BINS: usize = FRAME_LENGTH / 2 + 1;
/// sherpa-onnx: "1000 is an experience-value" tail padding for language identification.
const TAIL_PADDING_FRAMES: usize = 1000;
const MAX_FRAMES: usize = 3000;

fn vendor_err(message: impl Into<String>) -> SpeechError {
    SpeechError::Vendor {
        vendor: "local-sherpa".into(),
        message: message.into(),
    }
}

/// Whisper encoder/decoder sessions plus the language token table from the model metadata.
pub struct WhisperLanguageScorer {
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    sot: i64,
    n_mels: usize,
    n_text_layer: usize,
    n_text_ctx: usize,
    n_text_state: usize,
    language_tokens: Vec<i64>,
    language_codes: Vec<String>,
    mel_banks: Vec<Vec<f64>>,
}

/// Per-language decoder logits, aligned with [`WhisperLanguageScorer::language_codes`].
#[derive(Debug, Clone)]
pub struct LanguageScores {
    pub codes: Vec<String>,
    pub logits: Vec<f32>,
}

fn meta_usize(session: &Session, key: &str) -> SpeechResult<usize> {
    meta_string(session, key)?
        .trim()
        .parse()
        .map_err(|_| vendor_err(format!("Whisper metadata `{key}` is not a number")))
}

fn meta_string(session: &Session, key: &str) -> SpeechResult<String> {
    session
        .metadata()
        .map_err(|e| vendor_err(e.to_string()))?
        .custom(key)
        .map_err(|e| vendor_err(e.to_string()))?
        .ok_or_else(|| vendor_err(format!("Whisper encoder metadata has no `{key}`")))
}

fn open_session(path: &std::path::Path) -> SpeechResult<Session> {
    node_webrtc_rust_speech::ort_backend::ensure_ort_api().map_err(vendor_err)?;
    Session::builder()
        // Same intra-op thread count as the sherpa LID engine (`SHERPA_LID_NUM_THREADS`, default 1):
        // int8 kernels reduce in a thread-dependent order, so a different count can flip near-ties
        // and the scorer's top would disagree with sherpa's.
        .and_then(|b| b.with_intra_threads(crate::loader::lid_num_threads().max(1) as usize))
        .and_then(|b| b.with_inter_threads(1))
        .and_then(|b| b.commit_from_file(path))
        .map_err(|e| vendor_err(format!("open {}: {e}", path.display())))
}

impl WhisperLanguageScorer {
    pub fn load(paths: &LidModelPaths) -> SpeechResult<Self> {
        let encoder = open_session(&paths.encoder)?;
        let decoder = open_session(&paths.decoder)?;
        let n_mels = meta_usize(&encoder, "n_mels")?;
        let language_tokens = meta_string(&encoder, "all_language_tokens")?
            .split(',')
            .map(|t| t.trim().parse::<i64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| vendor_err("Whisper metadata `all_language_tokens` is malformed"))?;
        let language_codes: Vec<String> = meta_string(&encoder, "all_language_codes")?
            .split(',')
            .map(|c| c.trim().to_ascii_lowercase())
            .collect();
        if language_tokens.len() != language_codes.len() || language_codes.is_empty() {
            return Err(vendor_err(
                "Whisper metadata language tokens and codes differ in length",
            ));
        }
        Ok(Self {
            sot: meta_usize(&encoder, "sot")? as i64,
            n_text_layer: meta_usize(&encoder, "n_text_layer")?,
            n_text_ctx: meta_usize(&encoder, "n_text_ctx")?,
            n_text_state: meta_usize(&encoder, "n_text_state")?,
            mel_banks: slaney_mel_banks(n_mels),
            n_mels,
            language_tokens,
            language_codes,
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
        })
    }

    /// Loads the encoder/decoder `SpokenLanguageIdentification` would load for `config`.
    pub fn load_for_config(
        config: &node_webrtc_rust_speech::config::LanguageIdConfig,
    ) -> SpeechResult<Self> {
        Self::load(&crate::lid_model_paths::resolve_lid_model_paths(config)?)
    }

    pub fn language_codes(&self) -> &[String] {
        &self.language_codes
    }

    /// Language logits for 16 kHz mono samples in [-1, 1].
    pub fn language_scores(&self, samples: &[f32]) -> SpeechResult<LanguageScores> {
        let mel = normalized_log_mel(samples, &self.mel_banks);
        let num_frames = mel.len() / self.n_mels;
        if num_frames == 0 {
            return Err(vendor_err("Whisper LID: no audio frames"));
        }
        let total = (num_frames + TAIL_PADDING_FRAMES).min(MAX_FRAMES);
        let used = num_frames.min(MAX_FRAMES - 50);
        // (1, n_mels, total), zero padded; frames beyond `used` stay zero.
        let mut input = vec![0f32; self.n_mels * total];
        for t in 0..used {
            for m in 0..self.n_mels {
                input[m * total + t] = mel[t * self.n_mels + m];
            }
        }
        let mel_tensor = Tensor::from_array(([1usize, self.n_mels, total], input))
            .map_err(|e| vendor_err(e.to_string()))?;
        let (cross_k, cross_v) = {
            let mut enc = self
                .encoder
                .lock()
                .map_err(|_| SpeechError::Internal("whisper encoder lock poisoned".into()))?;
            let out = enc
                .run(ort::inputs![mel_tensor])
                .map_err(|e| vendor_err(format!("Whisper encoder: {e}")))?;
            let take = |i: usize| -> SpeechResult<(Vec<i64>, Vec<f32>)> {
                let (shape, data) = out[i]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| vendor_err(e.to_string()))?;
                Ok((shape.iter().copied().collect(), data.to_vec()))
            };
            (take(0)?, take(1)?)
        };
        let tensor = |(shape, data): (Vec<i64>, Vec<f32>)| {
            let dims: Vec<usize> = shape.iter().map(|d| *d as usize).collect();
            Tensor::from_array((dims, data)).map_err(|e| vendor_err(e.to_string()))
        };
        let cache_shape = [self.n_text_layer, 1, self.n_text_ctx, self.n_text_state];
        let cache_len = cache_shape.iter().product::<usize>();
        let self_k = Tensor::from_array((cache_shape, vec![0f32; cache_len]))
            .map_err(|e| vendor_err(e.to_string()))?;
        let self_v = Tensor::from_array((cache_shape, vec![0f32; cache_len]))
            .map_err(|e| vendor_err(e.to_string()))?;
        let tokens = Tensor::from_array(([1usize, 1], vec![self.sot]))
            .map_err(|e| vendor_err(e.to_string()))?;
        let offset =
            Tensor::from_array(([1usize], vec![0i64])).map_err(|e| vendor_err(e.to_string()))?;
        let mut dec = self
            .decoder
            .lock()
            .map_err(|_| SpeechError::Internal("whisper decoder lock poisoned".into()))?;
        let out = dec
            .run(ort::inputs![
                tokens,
                self_k,
                self_v,
                tensor(cross_k)?,
                tensor(cross_v)?,
                offset
            ])
            .map_err(|e| vendor_err(format!("Whisper decoder: {e}")))?;
        let (_, logits) = out[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| vendor_err(e.to_string()))?;
        let vocab = logits.len();
        let picked = self
            .language_tokens
            .iter()
            .map(|&tok| {
                let i = tok as usize;
                if i < vocab {
                    Ok(logits[i])
                } else {
                    Err(vendor_err(format!(
                        "language token {tok} outside vocab {vocab}"
                    )))
                }
            })
            .collect::<SpeechResult<Vec<f32>>>()?;
        Ok(LanguageScores {
            codes: self.language_codes.clone(),
            logits: picked,
        })
    }
}

/// librosa `filters.mel(sr=16000, n_fft=400, n_mels, htk=False, norm="slaney")`.
pub(crate) fn slaney_mel_banks(n_mels: usize) -> Vec<Vec<f64>> {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    let hz_to_mel = |f: f64| {
        if f >= min_log_hz {
            min_log_mel + (f / min_log_hz).ln() / logstep
        } else {
            f / f_sp
        }
    };
    let mel_to_hz = |m: f64| {
        if m >= min_log_mel {
            min_log_hz * (logstep * (m - min_log_mel)).exp()
        } else {
            m * f_sp
        }
    };
    let nyquist = SAMPLE_RATE / 2.0;
    let fft_freqs: Vec<f64> = (0..N_FFT_BINS)
        .map(|k| nyquist * k as f64 / (N_FFT_BINS - 1) as f64)
        .collect();
    let max_mel = hz_to_mel(nyquist);
    let mel_pts: Vec<f64> = (0..n_mels + 2)
        .map(|i| mel_to_hz(max_mel * i as f64 / (n_mels + 1) as f64))
        .collect();
    (0..n_mels)
        .map(|i| {
            let lower_w = mel_pts[i + 1] - mel_pts[i];
            let upper_w = mel_pts[i + 2] - mel_pts[i + 1];
            let enorm = 2.0 / (mel_pts[i + 2] - mel_pts[i]);
            fft_freqs
                .iter()
                .map(|&f| {
                    let lower = (f - mel_pts[i]) / lower_w;
                    let upper = (mel_pts[i + 2] - f) / upper_w;
                    lower.min(upper).max(0.0) * enorm
                })
                .collect()
        })
        .collect()
}

/// Whisper mel power spectrum (frames × n_mels), kaldi-native-fbank `WhisperFeatureComputer`.
pub(crate) fn mel_power(samples: &[f32], banks: &[Vec<f64>]) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return Vec::new();
    }
    let num_frames = (n + FRAME_SHIFT / 2) / FRAME_SHIFT;
    let window: Vec<f64> = (0..FRAME_LENGTH)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FRAME_LENGTH as f64).cos())
        .collect();
    let (cos_t, sin_t): (Vec<f64>, Vec<f64>) = (0..FRAME_LENGTH)
        .map(|j| {
            let a = 2.0 * std::f64::consts::PI * j as f64 / FRAME_LENGTH as f64;
            (a.cos(), a.sin())
        })
        .unzip();
    let n_mels = banks.len();
    let mut out = Vec::with_capacity(num_frames * n_mels);
    let mut frame = vec![0f64; FRAME_LENGTH];
    let mut power = vec![0f64; N_FFT_BINS];
    for f in 0..num_frames {
        let start = (f * FRAME_SHIFT + FRAME_SHIFT / 2) as i64 - (FRAME_LENGTH / 2) as i64;
        for (j, slot) in frame.iter_mut().enumerate() {
            let mut idx = start + j as i64;
            // Kaldi `snip_edges=false` reflection.
            while idx < 0 || idx >= n as i64 {
                if idx < 0 {
                    idx = -idx - 1;
                } else {
                    idx = 2 * n as i64 - 1 - idx;
                }
            }
            *slot = samples[idx as usize] as f64 * window[j];
        }
        for (k, p) in power.iter_mut().enumerate() {
            let (mut re, mut im) = (0.0, 0.0);
            for (j, x) in frame.iter().enumerate() {
                let t = (k * j) % FRAME_LENGTH;
                re += x * cos_t[t];
                im -= x * sin_t[t];
            }
            *p = re * re + im * im;
        }
        for bank in banks {
            out.push(bank.iter().zip(&power).map(|(w, p)| w * p).sum());
        }
    }
    out
}

/// `log10(max(x, 1e-10))`, clamp to `max - 8`, `(x + 4) / 4` (sherpa-onnx `NormalizeFeatures`).
pub(crate) fn normalized_log_mel(samples: &[f32], banks: &[Vec<f64>]) -> Vec<f32> {
    let mel = mel_power(samples, banks);
    let logs: Vec<f64> = mel.iter().map(|x| x.max(1e-10).log10()).collect();
    let max = logs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    logs.iter()
        .map(|x| ((x.max(max - 8.0) + 4.0) / 4.0) as f32)
        .collect()
}
