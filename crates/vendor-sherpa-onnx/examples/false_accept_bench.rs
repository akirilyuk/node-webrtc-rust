//! Speaker gate false-accept bench per voice pair (plan `2026-11-13-speaker-split-other-to-caller`,
//! chunk 1b, decision g): how often does a window of another voice score `caller` against an
//! enrolled caller, per (caller voice, other voice) pair, and which gate change cuts that without
//! failing real caller windows? Measurement only; the gate is not changed here.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example false_accept_bench -- \
//!   --voices-root <.models> --speaker-dir <.models/speaker> --bed <tocumen48.f32> \
//!   --cache <dir> --airport-a <scene A 16 kHz mono wav> --airport-b <scene B 16 kHz mono wav> \
//!   [--bed-db -12] [--out <file.md>] [--librispeech <LibriSpeech dir>]
//! ```
//!
//! With `--librispeech` (chunk 1c, decision j) the bench runs on real speech instead: see
//! [`librispeech`]. The LibriSpeech flac files must be decoded to wav next to them first
//! (`flac -d`); audio and embeddings stay outside the repo.
//!
//! - Voices: callers and others are the test set (`en_US-amy-low` sid 0 plus `TEST_VOICES`); the
//!   tune set (`TUNE_VOICES`) only picks thresholds. The impostor cohort for score normalization
//!   is 30 `en_US-libritts_r-medium` speakers that are in neither set ([`COHORT_SIDS`]).
//! - Per voice: an enrollment clip (3000 ms from the voiced onset, the gate's `enroll_speech_ms`;
//!   no pre-roll, as the gate's `enrollment_pcm_excludes_the_pre_roll`) from
//!   two long sentences, and a talk stream of 20 other sentences (200 ms gaps). Voices are
//!   normalized to -20 dBFS voiced RMS. Condition `bed`: the Tocumen airport bed (48 kHz mono f32le,
//!   resampled) at `--bed-db` re the voice under both enrollment and stream; condition `clean`: none.
//! - Windows: the stream is tiled in consecutive windows of `W` s (1.5 / 2.0 / 2.5 / 3.0), each
//!   clip with the gate's 300 ms pre-roll; windows less than half voiced (20 ms frames above -45
//!   dBFS on the clean voice) are skipped. A window is `caller` when its score >= the threshold.
//! - FAR(pair c, o) = share of o's windows accepted against c's enrollment; FRR(c) = share of c's
//!   own windows rejected. Same condition for enrollment and windows.
//! - Scores: `raw` cosine; `s-norm` = mean of the z-scores of the raw score against the cohort
//!   scores of the enrollment and of the window; `as-norm` the same over the top 10 cohort scores.
//!   `raw AND s-norm` / `raw AND as-norm`: accepted only when the raw cosine is >= 0.459 and the
//!   normalized score is >= its threshold (encoded as the normalized score, or [`REJECT`] when the
//!   raw cosine is below 0.459, so one threshold applies).
//! - Thresholds: today's gate is TitaNet-small raw at 0.459, also kept for longer TitaNet windows.
//!   Every other candidate's threshold is picked on the tune set: the highest threshold whose tune
//!   FRR is at most max(today's tune FRR, 1 %).
//! - Airport demo check: the gate's enrollment (scene 1.7-5.0 s) and its window grid for the two
//!   leaking utterances. The utterance start is fitted to the logged TitaNet 1.5 s scores
//!   ([`AIRPORT`]); each candidate then scores the clip of `W` s (+ pre-roll) that ends where the
//!   logged caller window ends, i.e. the window that would confirm at the same moment. The offline
//!   raw scores run higher than the logged ones, so 1.5 s candidates are also scored with the
//!   logged raw cosine in place of the reproduced one (cohort stats from the reproduced
//!   embeddings); a candidate passes the airport check only when both pass.
//! - Cohort statistics per other voice (TitaNet 1.5 s): mean window-side cohort mean / std, to show
//!   how much of the normalization comes from the cohort being the same TTS family as a voice.
//! - Embed time: p50 / p99 per window per model and window length (one thread).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

#[path = "support/mod.rs"]
mod support;

use support::{
    create_tts, db_gain, normalize_to, read_wav, resample_linear, rms_voiced_db, synthesize_sid,
    voice, Voice, TEST_VOICES, TUNE_VOICES,
};

const SR: i32 = 16_000;
const MS: usize = 16;
const PRE_ROLL_MS: usize = 300;
const ENROLL_MS: usize = 3000;
const GATE_THRESHOLD: f32 = 0.459;
const MAIN_DB: f32 = -20.0;
const WINDOWS_S: [f32; 4] = [1.5, 2.0, 2.5, 3.0];
const TOP_K: usize = 10;
const GAP_MS: usize = 200;
/// Score of a `raw AND …` window whose raw cosine is below the gate threshold.
const REJECT: f32 = -1.0e6;

const LONG: &str = include_str!("support/long_sentences.txt");
const LONG_BACKGROUND: &str = include_str!("support/long_background_sentences.txt");
const BACKGROUND: &str = include_str!("support/background_sentences.txt");

const AMY: Voice = voice("en_US-amy-low", "vits-piper-en_US-amy-low", 0);
const COHORT_SIDS: [i32; 30] = [
    5, 15, 30, 70, 90, 110, 140, 160, 180, 200, 220, 240, 260, 280, 320, 340, 360, 380, 400, 420,
    450, 500, 550, 600, 650, 700, 750, 800, 850, 900,
];

/// Airport demo utterances that leaked (VOICE_DEBUG logs of the split baselines, 0.18.4):
/// (label, approximate utterance start ms, logged TitaNet 1.5 s window scores, caller windows).
struct Airport {
    label: &'static str,
    approx_u_ms: usize,
    logged: &'static [f32],
    caller: &'static [usize],
}

const AIRPORT: [Airport; 2] = [
    Airport {
        label: "A utt-12 (key 3)",
        approx_u_ms: 22_000,
        logged: &[
            -0.058, 0.001, -0.014, -0.030, -0.076, 0.106, 0.220, 0.179, 0.697, 0.650,
        ],
        caller: &[9, 10],
    },
    Airport {
        label: "B utt-10 (key 4)",
        approx_u_ms: 35_900,
        logged: &[0.082, 0.021, 0.059, 0.040, 0.515, 0.514, 0.562],
        caller: &[5, 6, 7],
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Norm {
    Raw,
    Snorm,
    Asnorm,
    RawAndSnorm,
    RawAndAsnorm,
}

impl Norm {
    fn name(self) -> &'static str {
        match self {
            Norm::Raw => "raw",
            Norm::Snorm => "s-norm",
            Norm::Asnorm => "as-norm",
            Norm::RawAndSnorm => "raw AND s-norm",
            Norm::RawAndAsnorm => "raw AND as-norm",
        }
    }
}

struct Candidate {
    model: usize,
    window: usize,
    norm: Norm,
    /// Fixed threshold (today's gate); `None` = tune-matched.
    fixed: Option<f32>,
}

struct Embedder {
    name: String,
    extractor: SpeakerEmbeddingExtractor,
    times: HashMap<usize, Vec<f64>>,
}

impl Embedder {
    fn open(path: &Path) -> Result<Self, String> {
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(path.to_string_lossy().into_owned()),
            num_threads: 1,
            ..Default::default()
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config)
            .ok_or_else(|| format!("cannot open speaker model {}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            name,
            extractor,
            times: HashMap::new(),
        })
    }

    fn embed(&mut self, samples: &[f32], timing_key: Option<usize>) -> Result<Vec<f32>, String> {
        let start = Instant::now();
        let stream = self
            .extractor
            .create_stream()
            .ok_or("create_stream failed")?;
        stream.accept_waveform(SR, samples);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return Err(format!(
                "{}: clip too short ({} samples)",
                self.name,
                samples.len()
            ));
        }
        let e = self.extractor.compute(&stream).ok_or("compute failed")?;
        if let Some(key) = timing_key {
            self.times
                .entry(key)
                .or_default()
                .push(start.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(e)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

fn fnv1a(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

struct Synth {
    root: PathBuf,
    cache: PathBuf,
    engines: HashMap<&'static str, sherpa_onnx::OfflineTts>,
}

impl Synth {
    /// 16 kHz mono clip of `text` in `v`, cached on disk (VITS sampling is random per run).
    fn clip(&mut self, v: &Voice, text: &str) -> Result<Vec<f32>, String> {
        let key = format!("{}|{}|{}", v.bundle, v.sid, text);
        let path = self.cache.join(format!("{:016x}.f32", fnv1a(&key)));
        if let Ok(bytes) = fs::read(&path) {
            return Ok(bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect());
        }
        if !self.engines.contains_key(v.bundle) {
            eprintln!("loading TTS {}", v.bundle);
            self.engines
                .insert(v.bundle, create_tts(&self.root.join(v.bundle))?);
        }
        let (samples, rate) = synthesize_sid(&self.engines[v.bundle], text, v.sid)?;
        let samples = resample_linear(&samples, rate, SR);
        let bytes: Vec<u8> = samples.iter().flat_map(|x| x.to_le_bytes()).collect();
        fs::write(&path, bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(samples)
    }

    fn concat(&mut self, v: &Voice, texts: &[&str]) -> Result<Vec<f32>, String> {
        let mut out = Vec::new();
        for t in texts {
            out.extend(self.clip(v, t)?);
            out.extend(std::iter::repeat_n(0.0, GAP_MS * MS));
        }
        Ok(normalize_to(&out, MAIN_DB))
    }
}

/// `voice` plus the bed (starting at `offset`) at `bed_db` re the voice's voiced RMS.
fn with_bed(voice: &[f32], bed: &[f32], offset: usize, bed_db: f32) -> Vec<f32> {
    let bed_rms = {
        let s: f64 = bed.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
        (s / bed.len() as f64).sqrt() as f32
    };
    let target = db_gain(rms_voiced_db(voice, SR) + bed_db);
    let gain = if bed_rms > 0.0 { target / bed_rms } else { 0.0 };
    let mut out = voice.to_vec();
    for (i, x) in out.iter_mut().enumerate() {
        *x += bed[(offset + i) % bed.len()] * gain;
    }
    out
}

fn voiced_share(clean: &[f32]) -> f32 {
    let frames: Vec<f32> = clean
        .chunks(20 * MS)
        .map(|f| {
            let s: f32 = f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32;
            10.0 * s.max(1e-12).log10()
        })
        .collect();
    if frames.is_empty() {
        return 0.0;
    }
    frames.iter().filter(|db| **db > -45.0).count() as f32 / frames.len() as f32
}

/// Embeddings of one voice in one condition, per model: enrollment and windows per length.
struct VoiceEmb {
    enroll: Vec<Vec<f32>>,
    /// [model][window index] -> embeddings
    windows: Vec<Vec<Vec<Vec<f32>>>>,
}

struct Cond {
    bed: bool,
}

const CONDS: [Cond; 2] = [Cond { bed: true }, Cond { bed: false }];

/// Models embedding each window length: TitaNet (model 0) all lengths, the others 1.5 s only.
fn model_windows(model: usize) -> &'static [usize] {
    if model == 0 {
        &[0, 1, 2, 3]
    } else {
        &[0]
    }
}

#[allow(clippy::too_many_arguments)]
fn embed_voice(
    synth: &mut Synth,
    models: &mut [Embedder],
    v: &Voice,
    index: usize,
    bed: &[f32],
    bed_db: f32,
    wins: fn(usize) -> &'static [usize],
) -> Result<[VoiceEmb; 2], String> {
    let long = lines(LONG);
    let enroll_clean = synth.concat(v, &long[0..2])?;
    let mut texts: Vec<&str> = lines(LONG_BACKGROUND);
    texts.extend_from_slice(&long[8..16]);
    let stream_clean = synth.concat(v, &texts)?;
    embed_audio(
        models,
        &enroll_clean,
        &stream_clean,
        index,
        bed,
        bed_db,
        wins,
    )
}

/// Enrollment and window embeddings of one speaker from its clean enrollment and talk audio,
/// with and without the bed. `wins(model)` lists the window lengths ([`WINDOWS_S`] indices).
#[allow(clippy::too_many_arguments)]
fn embed_audio(
    models: &mut [Embedder],
    enroll_clean: &[f32],
    stream_clean: &[f32],
    index: usize,
    bed: &[f32],
    bed_db: f32,
    wins: fn(usize) -> &'static [usize],
) -> Result<[VoiceEmb; 2], String> {
    let mut out = Vec::new();
    for cond in &CONDS {
        let offset = index * 7 * SR as usize;
        let (enroll, stream) = if cond.bed {
            (
                with_bed(enroll_clean, bed, offset, bed_db),
                with_bed(stream_clean, bed, offset + 3 * SR as usize, bed_db),
            )
        } else {
            (enroll_clean.to_vec(), stream_clean.to_vec())
        };
        // The gate's enrollment: speech from the onset (first voiced frame), no pre-roll.
        let onset = enroll_clean
            .chunks(20 * MS)
            .position(|f| voiced_share(f) > 0.0)
            .unwrap_or(0)
            * 20
            * MS;
        let e0 = onset;
        let e1 = (onset + ENROLL_MS * MS).min(enroll.len());
        let mut ve = VoiceEmb {
            enroll: Vec::new(),
            windows: Vec::new(),
        };
        for (m, emb) in models.iter_mut().enumerate() {
            ve.enroll.push(emb.embed(&enroll[e0..e1], None)?);
            let mut per_w = vec![Vec::new(); WINDOWS_S.len()];
            for &wi in wins(m) {
                let w = (WINDOWS_S[wi] * 1000.0) as usize * MS;
                let pre = PRE_ROLL_MS * MS;
                let mut start = pre;
                while start + w <= stream.len() {
                    if voiced_share(&stream_clean[start..start + w]) >= 0.5 {
                        per_w[wi].push(emb.embed(&stream[start - pre..start + w], Some(wi))?);
                    }
                    start += w;
                }
            }
            ve.windows.push(per_w);
        }
        out.push(ve);
    }
    let clean = out.pop().ok_or("no clean")?;
    let bedv = out.pop().ok_or("no bed")?;
    Ok([bedv, clean])
}

/// Cohort: [cond][model] -> embeddings of a 3.3 s clip per cohort speaker.
type Cohort = Vec<Vec<Vec<Vec<f32>>>>;

/// The Piper cohort ([`COHORT_SIDS`] of `en_US-libritts_r-medium`).
fn piper_cohort(
    synth: &mut Synth,
    models: &mut [Embedder],
    bed: &[f32],
    bed_db: f32,
) -> Result<Cohort, String> {
    let bg = lines(BACKGROUND);
    let clips = COHORT_SIDS
        .iter()
        .enumerate()
        .map(|(i, sid)| {
            let v = voice(
                "en_US-libritts_r-medium",
                "vits-piper-en_US-libritts_r-medium",
                *sid,
            );
            let texts = [bg[(2 * i) % bg.len()], bg[(2 * i + 1) % bg.len()]];
            synth.concat(&v, &texts)
        })
        .collect::<Result<Vec<_>, _>>()?;
    cohort_from_clips(models, &clips, bed, bed_db)
}

/// Cohort embeddings: the first pre-roll + enrollment span of each clean clip, with and without
/// the bed.
fn cohort_from_clips(
    models: &mut [Embedder],
    clips: &[Vec<f32>],
    bed: &[f32],
    bed_db: f32,
) -> Result<Cohort, String> {
    let mut cohort: Cohort = vec![vec![Vec::new(); models.len()]; 2];
    for (i, clean) in clips.iter().enumerate() {
        let end = ((PRE_ROLL_MS + ENROLL_MS) * MS).min(clean.len());
        for (ci, cond) in CONDS.iter().enumerate() {
            let a = if cond.bed {
                with_bed(clean, bed, (40 + i) * 5 * SR as usize, bed_db)
            } else {
                clean.clone()
            };
            for (m, emb) in models.iter_mut().enumerate() {
                cohort[ci][m].push(emb.embed(&a[..end], None)?);
            }
        }
    }
    Ok(cohort)
}

/// The highest threshold whose FRR on `gen` is at most `target`: the score at index
/// `floor(target * n)` of the sorted genuine scores. When that index lands on a [`REJECT`] score
/// (a `raw AND …` window whose raw cosine is below the gate), no threshold reaches `target`; the
/// lowest real score is used instead, so the threshold never accepts a raw-rejected window.
fn tuned_threshold(gen: &[Vec<Vec<f32>>], target: f64) -> f32 {
    let mut all: Vec<f32> = gen.iter().flatten().flatten().copied().collect();
    if all.is_empty() {
        return f32::MAX;
    }
    all.sort_by(f32::total_cmp);
    let t = all[((target * all.len() as f64).floor() as usize).min(all.len() - 1)];
    if t > REJECT {
        t
    } else {
        all.iter()
            .copied()
            .find(|s| *s > REJECT)
            .unwrap_or(f32::MAX)
    }
}

fn cohort_stats(e: &[f32], cohort: &[Vec<f32>], top: Option<usize>) -> (f32, f32) {
    let mut s: Vec<f32> = cohort.iter().map(|c| cosine(e, c)).collect();
    s.sort_by(|a, b| b.total_cmp(a));
    if let Some(k) = top {
        s.truncate(k);
    }
    let n = s.len() as f32;
    let mean = s.iter().sum::<f32>() / n;
    let var = s.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    (mean, var.sqrt().max(1e-4))
}

fn score(norm: Norm, enroll: &[f32], window: &[f32], cohort: &[Vec<f32>]) -> f32 {
    score_from_raw(norm, cosine(enroll, window), enroll, window, cohort)
}

/// [`score`] with the raw cosine given (the airport check feeds the logged one).
fn score_from_raw(
    norm: Norm,
    raw: f32,
    enroll: &[f32],
    window: &[f32],
    cohort: &[Vec<f32>],
) -> f32 {
    let top = match norm {
        Norm::Raw => return raw,
        Norm::Snorm | Norm::RawAndSnorm => None,
        Norm::Asnorm | Norm::RawAndAsnorm => Some(TOP_K),
    };
    if matches!(norm, Norm::RawAndSnorm | Norm::RawAndAsnorm) && raw < GATE_THRESHOLD {
        return REJECT;
    }
    let (me, se) = cohort_stats(enroll, cohort, top);
    let (mt, st) = cohort_stats(window, cohort, top);
    0.5 * ((raw - me) / se + (raw - mt) / st)
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "-".into()
    } else {
        format!("{:.1}", 100.0 * n as f64 / d as f64)
    }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

struct Args {
    voices_root: PathBuf,
    speaker_dir: PathBuf,
    bed: PathBuf,
    cache: PathBuf,
    airport: [PathBuf; 2],
    bed_db: f32,
    out: Option<PathBuf>,
    /// `LibriSpeech/` root holding `SPEAKERS.TXT`, `test-clean/` and `dev-clean/` (decoded wav).
    librispeech: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut map = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().ok_or(format!("{k} needs a value"))?;
        map.insert(k, v);
    }
    let get = |k: &str| map.get(k).cloned().ok_or(format!("missing {k}"));
    Ok(Args {
        voices_root: get("--voices-root")?.into(),
        speaker_dir: get("--speaker-dir")?.into(),
        bed: get("--bed")?.into(),
        cache: get("--cache")?.into(),
        airport: [get("--airport-a")?.into(), get("--airport-b")?.into()],
        bed_db: map
            .get("--bed-db")
            .map(|s| s.parse::<f32>().map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or(-12.0),
        out: map.get("--out").map(PathBuf::from),
        librispeech: map.get("--librispeech").map(PathBuf::from),
    })
}

fn read_f32le(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}

fn clip(audio: &[f32], start_ms: usize, end_ms: usize) -> &[f32] {
    let a = (start_ms * MS).min(audio.len());
    let b = (end_ms * MS).min(audio.len());
    &audio[a..b]
}

/// Airport results per candidate: scores of the caller windows (`None` = could not fit).
struct AirportFit {
    label: &'static str,
    u_ms: usize,
    rmse: f32,
    reproduced: Vec<f32>,
    /// [candidate] -> scores of the caller windows
    scores: Vec<Vec<f32>>,
    /// [candidate] -> scores with the logged raw cosine (1.5 s candidates only)
    logged_scores: Vec<Option<Vec<f32>>>,
}

/// Today's gate reproduced offline on one airport utterance: the scene audio, the enrollment
/// embedding per model, and the utterance start fitted to the logged TitaNet 1.5 s scores
/// (model 0 must be TitaNet).
struct AirportBase {
    audio: Vec<f32>,
    refs: Vec<Vec<f32>>,
    u_ms: usize,
    rmse: f32,
    reproduced: Vec<f32>,
}

fn fit_airport(models: &mut [Embedder], ap: &Airport, path: &Path) -> Result<AirportBase, String> {
    let audio = read_wav(path)?;
    let enroll_clip = clip(&audio, 2000, 2000 + ENROLL_MS);
    let refs: Vec<Vec<f32>> = models
        .iter_mut()
        .map(|m| m.embed(enroll_clip, None))
        .collect::<Result<_, _>>()?;
    let mut best = (f32::MAX, 0usize, Vec::new());
    let mut u = ap.approx_u_ms - 1000;
    while u <= ap.approx_u_ms + 1000 {
        let mut rep = Vec::new();
        for k in 1..=ap.logged.len() {
            let s = u + (k - 1) * 1500;
            let e = models[0].embed(clip(&audio, s - PRE_ROLL_MS, s + 1500), None)?;
            rep.push(cosine(&refs[0], &e));
        }
        let sse: f32 = rep
            .iter()
            .zip(ap.logged)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        let rmse = (sse / rep.len() as f32).sqrt();
        if rmse < best.0 {
            best = (rmse, u, rep);
        }
        u += 50;
    }
    let (rmse, u_ms, reproduced) = best;
    Ok(AirportBase {
        audio,
        refs,
        u_ms,
        rmse,
        reproduced,
    })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    fs::create_dir_all(&args.cache).map_err(|e| e.to_string())?;
    if let Some(root) = &args.librispeech {
        return librispeech::run(&args, root);
    }
    let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
    let model_files = [
        "nemo_en_titanet_small.onnx",
        "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
        "wespeaker_en_voxceleb_resnet34_LM.onnx",
    ];
    let mut models = Vec::new();
    for f in model_files {
        models.push(Embedder::open(&args.speaker_dir.join(f))?);
    }
    let mut synth = Synth {
        root: args.voices_root.clone(),
        cache: args.cache.clone(),
        engines: HashMap::new(),
    };

    let mut test_voices = vec![AMY];
    test_voices.extend_from_slice(&TEST_VOICES);
    let tune_voices: Vec<Voice> = TUNE_VOICES.to_vec();
    let vname = |v: &Voice| {
        let short = v
            .name
            .trim_start_matches("en_US-")
            .trim_end_matches("-medium");
        if v.bundle.contains("libritts") {
            format!("{short} {}", v.sid)
        } else {
            short.to_string()
        }
    };

    let mut test = Vec::new();
    for (i, v) in test_voices.iter().enumerate() {
        eprintln!("test voice {}", vname(v));
        test.push(embed_voice(
            &mut synth,
            &mut models,
            v,
            i,
            &bed,
            args.bed_db,
            model_windows,
        )?);
    }
    let mut tune = Vec::new();
    for (i, v) in tune_voices.iter().enumerate() {
        eprintln!("tune voice {}", vname(v));
        tune.push(embed_voice(
            &mut synth,
            &mut models,
            v,
            10 + i,
            &bed,
            args.bed_db,
            model_windows,
        )?);
    }
    let cohort = piper_cohort(&mut synth, &mut models, &bed, args.bed_db)?;

    let mut cands = vec![Candidate {
        model: 0,
        window: 0,
        norm: Norm::Raw,
        fixed: Some(GATE_THRESHOLD),
    }];
    for w in 1..WINDOWS_S.len() {
        cands.push(Candidate {
            model: 0,
            window: w,
            norm: Norm::Raw,
            fixed: Some(GATE_THRESHOLD),
        });
    }
    for w in 1..WINDOWS_S.len() {
        cands.push(Candidate {
            model: 0,
            window: w,
            norm: Norm::Raw,
            fixed: None,
        });
    }
    for w in [0, 1] {
        for norm in [
            Norm::Snorm,
            Norm::Asnorm,
            Norm::RawAndSnorm,
            Norm::RawAndAsnorm,
        ] {
            cands.push(Candidate {
                model: 0,
                window: w,
                norm,
                fixed: None,
            });
        }
    }
    for m in 1..models.len() {
        for norm in [Norm::Raw, Norm::Asnorm] {
            cands.push(Candidate {
                model: m,
                window: 0,
                norm,
                fixed: None,
            });
        }
    }
    let cand_name = |c: &Candidate, models: &[Embedder]| {
        let short = match c.model {
            0 => "titanet",
            1 => "campplus",
            _ => "resnet34",
        };
        let _ = models;
        format!(
            "{short} {}s {}{}",
            WINDOWS_S[c.window],
            c.norm.name(),
            if c.fixed.is_some() { " @0.459" } else { "" }
        )
    };

    // Scores of one set: genuine[cond] (per caller), impostor[cond][caller][other].
    type Scores = (Vec<Vec<Vec<f32>>>, Vec<Vec<Vec<Vec<f32>>>>);
    let set_scores = |set: &[[VoiceEmb; 2]], c: &Candidate| -> Scores {
        let mut gen = vec![vec![Vec::new(); set.len()]; 2];
        let mut imp = vec![vec![vec![Vec::new(); set.len()]; set.len()]; 2];
        for ci in 0..2 {
            for (a, va) in set.iter().enumerate() {
                let e = &va[ci].enroll[c.model];
                for (b, vb) in set.iter().enumerate() {
                    for w in &vb[ci].windows[c.model][c.window] {
                        let s = score(c.norm, e, w, &cohort[ci][c.model]);
                        if a == b {
                            gen[ci][a].push(s);
                        } else {
                            imp[ci][a][b].push(s);
                        }
                    }
                }
            }
        }
        (gen, imp)
    };
    let today_tune = set_scores(&tune, &cands[0]);
    let tune_frr = |gen: &[Vec<Vec<f32>>], t: f32| {
        let all: Vec<f32> = gen.iter().flatten().flatten().copied().collect();
        all.iter().filter(|s| **s < t).count() as f64 / all.len().max(1) as f64
    };
    let target = tune_frr(&today_tune.0, GATE_THRESHOLD).max(0.01);

    // Airport: fit the utterance start on today's gate, then score each candidate.
    let mut airport = Vec::new();
    for (ai, ap) in AIRPORT.iter().enumerate() {
        let AirportBase {
            audio,
            refs,
            u_ms,
            rmse,
            reproduced,
        } = fit_airport(&mut models, ap, &args.airport[ai])?;
        let mut scores = Vec::new();
        let mut logged_scores = Vec::new();
        for c in &cands {
            let mut row = Vec::new();
            let mut logged_row = Vec::new();
            for &k in ap.caller {
                let end = u_ms + k * 1500;
                let w = (WINDOWS_S[c.window] * 1000.0) as usize;
                let e = models[c.model].embed(clip(&audio, end - w - PRE_ROLL_MS, end), None)?;
                let cohort_m = &cohort[0][c.model];
                row.push(score(c.norm, &refs[c.model], &e, cohort_m));
                logged_row.push(score_from_raw(
                    c.norm,
                    ap.logged[k - 1],
                    &refs[c.model],
                    &e,
                    cohort_m,
                ));
            }
            scores.push(row);
            logged_scores.push((c.model == 0 && c.window == 0).then_some(logged_row));
        }
        airport.push(AirportFit {
            label: ap.label,
            u_ms,
            rmse,
            reproduced,
            scores,
            logged_scores,
        });
    }

    let mut md = String::new();
    md.push_str(&format!(
        "Bed: Tocumen airport CC0 at {} dB re the voice (`bed`), or none (`clean`). Voices at {MAIN_DB} dBFS. \
         Test callers/others: {}. Tune: {}. Cohort: libritts_r sids {:?}. Thresholds: `@0.459` fixed, \
         else tune-matched (tune FRR <= {:.1} %).\n\n",
        args.bed_db,
        test_voices.iter().map(vname).collect::<Vec<_>>().join(", "),
        tune_voices.iter().map(vname).collect::<Vec<_>>().join(", "),
        COHORT_SIDS,
        target * 100.0
    ));

    let mut pair_rows: Vec<Vec<String>> = Vec::new();
    let mut frr_rows: Vec<Vec<String>> = Vec::new();
    md.push_str("### Summary per candidate\n\n| candidate | threshold | test FAR bed / clean % | worst pair FAR bed % | FAR, other = libritts_r bed / clean % | test FRR bed / clean % | tune FAR / FRR % (pooled) | airport A w9, w10 | airport B w5, w6, w7 | airport A / B at logged raw | pass reproduced / at logged raw | airport pass | embed ms p50 / p99 | extra confirm delay s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for (ci, c) in cands.iter().enumerate() {
        let (tg, ti) = set_scores(&tune, c);
        let thr = match c.fixed {
            Some(t) => t,
            None => tuned_threshold(&tg, target),
        };
        let (gen, imp) = set_scores(&test, c);
        let count = |v: &[f32], acc: bool| {
            v.iter()
                .filter(|s| if acc { **s >= thr } else { **s < thr })
                .count()
        };
        let mut far_tot = [(0, 0); 2];
        let mut lib_tot = [(0, 0); 2];
        let mut frr_tot = [(0, 0); 2];
        let mut worst = (0.0f64, String::new());
        for a in 0..test.len() {
            for b in 0..test.len() {
                if a == b {
                    continue;
                }
                if ci == 0 {
                    pair_rows.push(vec![format!(
                        "{} ← {}",
                        vname(&test_voices[a]),
                        vname(&test_voices[b])
                    )]);
                }
                let mut cells = Vec::new();
                for k in 0..2 {
                    let v = &imp[k][a][b];
                    let n = count(v, true);
                    far_tot[k].0 += n;
                    far_tot[k].1 += v.len();
                    if test_voices[b].bundle.contains("libritts") {
                        lib_tot[k].0 += n;
                        lib_tot[k].1 += v.len();
                    }
                    if k == 0 {
                        let f = n as f64 / v.len().max(1) as f64;
                        if f > worst.0 {
                            worst = (
                                f,
                                format!("{} ← {}", vname(&test_voices[a]), vname(&test_voices[b])),
                            );
                        }
                    }
                    cells.push(pct(n, v.len()));
                }
                let row = a * (test.len() - 1) + if b > a { b - 1 } else { b };
                pair_rows[row].push(cells.join(" / "));
            }
            if ci == 0 {
                frr_rows.push(vec![vname(&test_voices[a])]);
            }
            let mut cells = Vec::new();
            for k in 0..2 {
                let v = &gen[k][a];
                let n = count(v, false);
                frr_tot[k].0 += n;
                frr_tot[k].1 += v.len();
                cells.push(pct(n, v.len()));
            }
            frr_rows[a].push(cells.join(" / "));
        }
        let tune_far: Vec<f32> = ti.iter().flatten().flatten().flatten().copied().collect();
        let tune_gen: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
        let fmt_air = |s: &[f32]| {
            s.iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pass_rep = airport
            .iter()
            .all(|a| a.scores[ci].iter().all(|s| *s >= thr));
        let pass_logged = airport.iter().all(|a| {
            a.logged_scores[ci]
                .as_ref()
                .is_none_or(|v| v.iter().all(|s| *s >= thr))
        });
        let pass = pass_rep && pass_logged;
        let yes_no = |b: bool| if b { "yes" } else { "**no**" };
        let logged_cell = match (&airport[0].logged_scores[ci], &airport[1].logged_scores[ci]) {
            (Some(a), Some(b)) => format!("{} / {}", fmt_air(a), fmt_air(b)),
            _ => "-".to_string(),
        };
        let pass_cell = if airport[0].logged_scores[ci].is_some() {
            format!("{} / {}", yes_no(pass_rep), yes_no(pass_logged))
        } else {
            format!("{} / -", yes_no(pass_rep))
        };
        let mut times = models[c.model]
            .times
            .get(&c.window)
            .cloned()
            .unwrap_or_default();
        let p50 = percentile(&mut times, 0.5);
        let p99 = percentile(&mut times, 0.99);
        md.push_str(&format!(
            "| {} | {:.3} | {} / {} | {:.1} ({}) | {} / {} | {} / {} | {} / {} | {} | {} | {} | {} | {} | {:.1} / {:.1} | {:.1} |\n",
            cand_name(c, &models),
            thr,
            pct(far_tot[0].0, far_tot[0].1),
            pct(far_tot[1].0, far_tot[1].1),
            worst.0 * 100.0,
            worst.1,
            pct(lib_tot[0].0, lib_tot[0].1),
            pct(lib_tot[1].0, lib_tot[1].1),
            pct(frr_tot[0].0, frr_tot[0].1),
            pct(frr_tot[1].0, frr_tot[1].1),
            pct(count(&tune_far, true), tune_far.len()),
            pct(count(&tune_gen, false), tune_gen.len()),
            fmt_air(&airport[0].scores[ci]),
            fmt_air(&airport[1].scores[ci]),
            logged_cell,
            pass_cell,
            yes_no(pass),
            p50,
            p99,
            WINDOWS_S[c.window] - 1.5
        ));
    }
    let header: Vec<String> = cands.iter().map(|c| cand_name(c, &models)).collect();
    let table = |title: &str, first: &str, rows: &[Vec<String>]| {
        let mut s = format!(
            "\n### {title}\n\n| {first} | {} |\n|---|{}\n",
            header.join(" | "),
            "---|".repeat(header.len())
        );
        for r in rows {
            s.push_str(&format!("| {} |\n", r.join(" | ")));
        }
        s
    };
    md.push_str(&table(
        "FAR per pair (caller ← other), bed / clean %",
        "pair",
        &pair_rows,
    ));
    md.push_str(&table("FRR per caller, bed / clean %", "caller", &frr_rows));
    md.push_str("\n### Cohort statistics per voice (TitaNet 1.5 s, libritts_r cohort)\n\nMean over the voice's windows of the window-side cohort mean / std (`μt` / `σt`), and the enrollment-side cohort mean / std (`μe` / `σe`).\n\n| voice | μt / σt bed | μt / σt clean | μe / σe bed | μe / σe clean |\n|---|---|---|---|---|\n");
    for (vi, v) in test_voices.iter().enumerate() {
        let mut cells = Vec::new();
        for ci in 0..2 {
            let ws = &test[vi][ci].windows[0][0];
            let (mut sm, mut ss) = (0.0f32, 0.0f32);
            for w in ws {
                let (m, sd) = cohort_stats(w, &cohort[ci][0], None);
                sm += m;
                ss += sd;
            }
            let n = ws.len().max(1) as f32;
            cells.push(format!("{:.3} / {:.3}", sm / n, ss / n));
        }
        for ci in 0..2 {
            let (m, sd) = cohort_stats(&test[vi][ci].enroll[0], &cohort[ci][0], None);
            cells.push(format!("{m:.3} / {sd:.3}"));
        }
        md.push_str(&format!("| {} | {} |\n", vname(v), cells.join(" | ")));
    }
    md.push_str("\n### Airport fit (today's gate reproduced offline)\n\n| utterance | fitted start ms | RMSE vs log | reproduced 1.5 s scores |\n|---|---|---|---|\n");
    for a in &airport {
        md.push_str(&format!(
            "| {} | {} | {:.3} | {} |\n",
            a.label,
            a.u_ms,
            a.rmse,
            a.reproduced
                .iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    print!("{md}");
    if let Some(out) = &args.out {
        fs::write(out, &md).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("false_accept_bench: {e}");
        std::process::exit(1);
    }
}

/// Chunk 1c (decision j): the same measurement on real speech. LibriSpeech (openslr SLR12,
/// CC BY 4.0) `test-clean` speakers are the callers and others, half-split `dev-clean` speakers
/// are the tune set and the impostor cohort (disjoint, sex-balanced). TitaNet-small only, 1.5 s
/// and 2.0 s windows. The Piper test voices and the airport demo are re-scored as cross-checks,
/// and the normalized candidates are also run with the Piper cohort to answer whether the
/// cohort asset can be synthetic.
mod librispeech {
    use super::*;

    /// Speech per speaker: enrollment from the first chapter (at least this long before the
    /// 3000 ms clip is cut from its voiced onset), talk stream from the other chapters.
    const ENROLL_SRC_S: usize = 5;
    const STREAM_S: usize = 120;
    /// `dev-clean` split per sex (it has 20 + 20 speakers): tune, then cohort.
    const TUNE_PER_SEX: usize = 5;
    const COHORT_PER_SEX: usize = 15;

    struct Speaker {
        id: String,
        female: bool,
        /// chapter -> utterance wav files, sorted
        chapters: Vec<Vec<PathBuf>>,
    }

    impl Speaker {
        fn label(&self) -> String {
            format!("{} {}", self.id, if self.female { "F" } else { "M" })
        }
    }

    fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut out: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| format!("read {}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        out.sort_by_key(|p| {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name.parse::<u64>().unwrap_or(u64::MAX), name)
        });
        Ok(out)
    }

    fn speakers(root: &Path, subset: &str) -> Result<Vec<Speaker>, String> {
        let sexes: HashMap<String, bool> = fs::read_to_string(root.join("SPEAKERS.TXT"))
            .map_err(|e| format!("SPEAKERS.TXT: {e}"))?
            .lines()
            .filter(|l| !l.starts_with(';'))
            .filter_map(|l| {
                let mut f = l.split('|').map(str::trim);
                Some((f.next()?.to_string(), f.next()? == "F"))
            })
            .collect();
        let mut out = Vec::new();
        for spk in sorted_dirs(&root.join(subset))? {
            if !spk.is_dir() {
                continue;
            }
            let id = spk
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let female = *sexes
                .get(&id)
                .ok_or(format!("speaker {id} not in SPEAKERS.TXT"))?;
            let mut chapters = Vec::new();
            for ch in sorted_dirs(&spk)? {
                if !ch.is_dir() {
                    continue;
                }
                let mut files: Vec<PathBuf> = fs::read_dir(&ch)
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "wav"))
                    .collect();
                files.sort();
                if !files.is_empty() {
                    chapters.push(files);
                }
            }
            if chapters.is_empty() {
                return Err(format!("{subset}/{id}: no decoded wav files"));
            }
            out.push(Speaker {
                id,
                female,
                chapters,
            });
        }
        Ok(out)
    }

    /// (enrollment source, talk stream), each utterances joined with 200 ms gaps at -20 dBFS.
    fn audio(sp: &Speaker) -> Result<(Vec<f32>, Vec<f32>), String> {
        let gap = vec![0.0f32; GAP_MS * MS];
        let mut enroll = Vec::new();
        let mut used = 0;
        for f in &sp.chapters[0] {
            if enroll.len() >= ENROLL_SRC_S * SR as usize {
                break;
            }
            enroll.extend(read_wav(f)?);
            enroll.extend_from_slice(&gap);
            used += 1;
        }
        let mut rest: Vec<&PathBuf> = sp.chapters[1..].iter().flatten().collect();
        rest.extend(sp.chapters[0][used..].iter());
        let mut stream = Vec::new();
        for f in rest {
            if stream.len() >= STREAM_S * SR as usize {
                break;
            }
            stream.extend(read_wav(f)?);
            stream.extend_from_slice(&gap);
        }
        Ok((
            normalize_to(&enroll, MAIN_DB),
            normalize_to(&stream, MAIN_DB),
        ))
    }

    fn windows_15_20(_model: usize) -> &'static [usize] {
        &[0, 1]
    }

    struct Cand {
        window: usize,
        norm: Norm,
        fixed: Option<f32>,
        piper_cohort: bool,
    }

    impl Cand {
        fn name(&self) -> String {
            format!(
                "{}s {}{}{}",
                WINDOWS_S[self.window],
                self.norm.name(),
                if self.fixed.is_some() { " @0.459" } else { "" },
                if self.piper_cohort {
                    " (Piper cohort)"
                } else {
                    ""
                }
            )
        }
    }

    type Scores = (Vec<Vec<Vec<f32>>>, Vec<Vec<Vec<Vec<f32>>>>);

    /// genuine[cond][caller], impostor[cond][caller][other] for TitaNet (model 0).
    fn set_scores(set: &[[VoiceEmb; 2]], c: &Cand, cohort: &Cohort) -> Scores {
        let mut gen = vec![vec![Vec::new(); set.len()]; 2];
        let mut imp = vec![vec![vec![Vec::new(); set.len()]; set.len()]; 2];
        for ci in 0..2 {
            for (a, va) in set.iter().enumerate() {
                let e = &va[ci].enroll[0];
                for (b, vb) in set.iter().enumerate() {
                    for w in &vb[ci].windows[0][c.window] {
                        let s = score(c.norm, e, w, &cohort[ci][0]);
                        if a == b {
                            gen[ci][a].push(s);
                        } else {
                            imp[ci][a][b].push(s);
                        }
                    }
                }
            }
        }
        (gen, imp)
    }

    fn rate(v: &[f32], thr: f32, accept: bool) -> (usize, usize) {
        let n = v
            .iter()
            .filter(|s| if accept { **s >= thr } else { **s < thr })
            .count();
        (n, v.len())
    }

    fn q(sorted: &[f64], p: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        sorted[((sorted.len() - 1) as f64 * p).round() as usize]
    }

    fn fmt_list(s: &[f32]) -> String {
        s.iter()
            .map(|x| format!("{x:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn run(args: &Args, root: &Path) -> Result<(), String> {
        let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
        let mut models = vec![Embedder::open(
            &args.speaker_dir.join("nemo_en_titanet_small.onnx"),
        )?];
        let test_sp = speakers(root, "test-clean")?;
        let dev = speakers(root, "dev-clean")?;
        let mut tune_sp = Vec::new();
        let mut cohort_sp = Vec::new();
        for female in [true, false] {
            let g: Vec<&Speaker> = dev.iter().filter(|s| s.female == female).collect();
            if g.len() < TUNE_PER_SEX + COHORT_PER_SEX {
                return Err(format!(
                    "dev-clean has {} speakers of sex {}, need {}",
                    g.len(),
                    if female { "F" } else { "M" },
                    TUNE_PER_SEX + COHORT_PER_SEX
                ));
            }
            tune_sp.extend_from_slice(&g[..TUNE_PER_SEX]);
            cohort_sp.extend_from_slice(&g[TUNE_PER_SEX..TUNE_PER_SEX + COHORT_PER_SEX]);
        }

        let mut test = Vec::new();
        let mut stream_s = Vec::new();
        for (i, sp) in test_sp.iter().enumerate() {
            eprintln!("test speaker {}", sp.label());
            let (e, s) = audio(sp)?;
            stream_s.push(s.len() as f64 / f64::from(SR));
            test.push(embed_audio(
                &mut models,
                &e,
                &s,
                i,
                &bed,
                args.bed_db,
                windows_15_20,
            )?);
        }
        let mut tune = Vec::new();
        for (i, sp) in tune_sp.iter().enumerate() {
            eprintln!("tune speaker {}", sp.label());
            let (e, s) = audio(sp)?;
            tune.push(embed_audio(
                &mut models,
                &e,
                &s,
                60 + i,
                &bed,
                args.bed_db,
                windows_15_20,
            )?);
        }
        let ls_clips = cohort_sp
            .iter()
            .map(|sp| audio(sp).map(|(e, _)| e))
            .collect::<Result<Vec<_>, _>>()?;
        let ls_cohort = cohort_from_clips(&mut models, &ls_clips, &bed, args.bed_db)?;
        let mut synth = Synth {
            root: args.voices_root.clone(),
            cache: args.cache.clone(),
            engines: HashMap::new(),
        };
        let p_cohort = piper_cohort(&mut synth, &mut models, &bed, args.bed_db)?;
        let mut piper_voices = vec![AMY];
        piper_voices.extend_from_slice(&TEST_VOICES);
        let mut piper = Vec::new();
        for (i, v) in piper_voices.iter().enumerate() {
            eprintln!("piper voice {}", v.name);
            piper.push(embed_voice(
                &mut synth,
                &mut models,
                v,
                i,
                &bed,
                args.bed_db,
                windows_15_20,
            )?);
        }
        let pname = |v: &Voice| {
            let short = v
                .name
                .trim_start_matches("en_US-")
                .trim_end_matches("-medium");
            if v.bundle.contains("libritts") {
                format!("{short} {}", v.sid)
            } else {
                short.to_string()
            }
        };
        let mut airport = Vec::new();
        for (ai, ap) in AIRPORT.iter().enumerate() {
            airport.push(fit_airport(&mut models, ap, &args.airport[ai])?);
        }

        let mut cands = vec![
            Cand {
                window: 0,
                norm: Norm::Raw,
                fixed: Some(GATE_THRESHOLD),
                piper_cohort: false,
            },
            Cand {
                window: 1,
                norm: Norm::Raw,
                fixed: Some(GATE_THRESHOLD),
                piper_cohort: false,
            },
        ];
        for w in [0, 1] {
            cands.push(Cand {
                window: w,
                norm: Norm::Raw,
                fixed: None,
                piper_cohort: false,
            });
        }
        for w in [0, 1] {
            for norm in [
                Norm::Snorm,
                Norm::Asnorm,
                Norm::RawAndSnorm,
                Norm::RawAndAsnorm,
            ] {
                cands.push(Cand {
                    window: w,
                    norm,
                    fixed: None,
                    piper_cohort: false,
                });
            }
        }
        for norm in [
            Norm::Snorm,
            Norm::Asnorm,
            Norm::RawAndSnorm,
            Norm::RawAndAsnorm,
        ] {
            cands.push(Cand {
                window: 0,
                norm,
                fixed: None,
                piper_cohort: true,
            });
        }

        let today_tune = set_scores(&tune, &cands[0], &ls_cohort);
        let tune_all: Vec<f32> = today_tune.0.iter().flatten().flatten().copied().collect();
        let today_tune_frr = rate(&tune_all, GATE_THRESHOLD, false);
        let target = (today_tune_frr.0 as f64 / today_tune_frr.1.max(1) as f64).max(0.01);
        let n_test = test.len();
        let n_pairs = n_test * (n_test - 1);
        let win_counts = |wi: usize| {
            let v: Vec<usize> = test.iter().map(|t| t[0].windows[0][wi].len()).collect();
            (
                v.iter().sum::<usize>() as f64 / v.len() as f64,
                *v.iter().min().unwrap_or(&0),
            )
        };
        let (w15, w15min) = win_counts(0);
        let (w20, w20min) = win_counts(1);
        let mut times0 = models[0].times.get(&0).cloned().unwrap_or_default();
        let mut times1 = models[0].times.get(&1).cloned().unwrap_or_default();

        let mut md = String::new();
        md.push_str(&format!(
            "Test: LibriSpeech `test-clean`, {} speakers ({} F / {} M), each caller and other. Tune: \
             `dev-clean` {} speakers ({}). Cohort: `dev-clean` {} other speakers ({}), disjoint from \
             tune and test. Per speaker: enrollment = 3000 ms from the voiced onset of the first \
             chapter's utterances, talk stream = {STREAM_S} s from the other chapters (mean {:.0} s). \
             Windows per speaker and condition: mean {w15:.0} (min {w15min}) at 1.5 s, {w20:.0} (min \
             {w20min}) at 2.0 s; {n_pairs} (caller, other) pairs. Bed: Tocumen airport at {} dB \
             (`bed`) or none (`clean`). Thresholds: `@0.459` fixed, else tune-matched: highest \
             threshold with tune FRR <= {:.1} % (today's gate's tune FRR {}, floor 1 %). TitaNet \
             embed p50 / p99: {:.1} / {:.1} ms (1.5 s), {:.1} / {:.1} ms (2.0 s).\n\n",
            n_test,
            test_sp.iter().filter(|s| s.female).count(),
            test_sp.iter().filter(|s| !s.female).count(),
            tune_sp.len(),
            tune_sp.iter().map(|s| s.label()).collect::<Vec<_>>().join(", "),
            cohort_sp.len(),
            cohort_sp
                .iter()
                .map(|s| s.label())
                .collect::<Vec<_>>()
                .join(", "),
            stream_s.iter().sum::<f64>() / stream_s.len() as f64,
            args.bed_db,
            target * 100.0,
            pct(today_tune_frr.0, today_tune_frr.1),
            percentile(&mut times0, 0.5),
            percentile(&mut times0, 0.99),
            percentile(&mut times1, 0.5),
            percentile(&mut times1, 0.99),
        ));

        md.push_str("### Real speech: summary per candidate\n\nPer-pair FAR distribution over the pairs (bed); FRR distribution over the 40 callers.\n\n| candidate | threshold | test FAR bed / clean % | same-sex FAR bed % | pairs with FAR > 0, bed | pair FAR p50 / p90 / p99 / max bed % | pairs >= 5 % / >= 20 % bed | pair FAR max clean % | test FRR bed / clean % | caller FRR p90 / max bed % | callers FRR > 5 % bed / clean | tune FAR / FRR % | airport min margin reproduced / logged raw | airport pass | extra confirm delay s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        let mut worst_md = String::from("\n### Real speech: worst 10 pairs per candidate (caller ← other, bed / clean %)\n\n| candidate | worst pairs |\n|---|---|\n");
        let mut piper_md = String::from("\n### Cross-check: Piper test voices scored with the real-speech thresholds and cohort\n\nSame windows as chunk 1b (bed / clean %).\n\n| candidate | threshold | Piper FAR bed / clean | amy-low ← libritts_r 120 | libritts_r 120 ← amy-low | libritts_r 300 ← lessac | lessac ← hfc_female | worst pair bed | FRR libritts_r 120 | FRR libritts_r 300 | max FRR other callers bed / clean |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
        let mut air_md = String::from("\n### Cross-check: airport demo caller windows\n\nReproduced offline (enrollment and window embeddings) and at the logged raw cosine (1.5 s only; cohort statistics from the reproduced embeddings). Cohort as in the candidate; bed condition cohort.\n\n| candidate | threshold | A w9, w10 reproduced | B w5, w6, w7 reproduced | A / B at logged raw | min margin reproduced / logged |\n|---|---|---|---|---|---|\n");

        for c in &cands {
            let cohort = if c.piper_cohort {
                &p_cohort
            } else {
                &ls_cohort
            };
            let (tg, ti) = set_scores(&tune, c, cohort);
            let thr = c.fixed.unwrap_or_else(|| tuned_threshold(&tg, target));
            let (gen, imp) = set_scores(&test, c, cohort);
            let mut far_tot = [(0usize, 0usize); 2];
            let mut same_tot = (0usize, 0usize);
            let mut pair_far: Vec<Vec<(f64, usize, usize)>> = vec![Vec::new(); 2];
            for k in 0..2 {
                for a in 0..n_test {
                    for b in 0..n_test {
                        if a == b {
                            continue;
                        }
                        let (n, d) = rate(&imp[k][a][b], thr, true);
                        far_tot[k].0 += n;
                        far_tot[k].1 += d;
                        if k == 0 && test_sp[a].female == test_sp[b].female {
                            same_tot.0 += n;
                            same_tot.1 += d;
                        }
                        pair_far[k].push((n as f64 / d.max(1) as f64, a, b));
                    }
                }
            }
            let mut bed_sorted: Vec<f64> = pair_far[0].iter().map(|p| p.0 * 100.0).collect();
            bed_sorted.sort_by(f64::total_cmp);
            let clean_max = pair_far[1].iter().map(|p| p.0 * 100.0).fold(0.0, f64::max);
            let nz = bed_sorted.iter().filter(|x| **x > 0.0).count();
            let ge5 = bed_sorted.iter().filter(|x| **x >= 5.0).count();
            let ge20 = bed_sorted.iter().filter(|x| **x >= 20.0).count();
            let mut frr_tot = [(0usize, 0usize); 2];
            let mut caller_frr: Vec<Vec<f64>> = vec![Vec::new(); 2];
            for k in 0..2 {
                for g in &gen[k] {
                    let (n, d) = rate(g, thr, false);
                    frr_tot[k].0 += n;
                    frr_tot[k].1 += d;
                    caller_frr[k].push(100.0 * n as f64 / d.max(1) as f64);
                }
                caller_frr[k].sort_by(f64::total_cmp);
            }
            let over5 = |k: usize| caller_frr[k].iter().filter(|x| **x > 5.0).count();
            let tune_far: Vec<f32> = ti.iter().flatten().flatten().flatten().copied().collect();
            let tune_gen: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
            let (tfa, tfd) = rate(&tune_far, thr, true);
            let (tra, trd) = rate(&tune_gen, thr, false);

            // Airport.
            let mut rep_scores = Vec::new();
            let mut log_scores = Vec::new();
            for (ai, ap) in AIRPORT.iter().enumerate() {
                let base = &airport[ai];
                let mut row = Vec::new();
                let mut lrow = Vec::new();
                for &k in ap.caller {
                    let end = base.u_ms + k * 1500;
                    let w = (WINDOWS_S[c.window] * 1000.0) as usize;
                    let e = models[0].embed(clip(&base.audio, end - w - PRE_ROLL_MS, end), None)?;
                    row.push(score(c.norm, &base.refs[0], &e, &cohort[0][0]));
                    lrow.push(score_from_raw(
                        c.norm,
                        ap.logged[k - 1],
                        &base.refs[0],
                        &e,
                        &cohort[0][0],
                    ));
                }
                rep_scores.push(row);
                log_scores.push(lrow);
            }
            let min_margin = |v: &[Vec<f32>]| {
                v.iter()
                    .flatten()
                    .map(|s| s - thr)
                    .fold(f32::INFINITY, f32::min)
            };
            let m_rep = min_margin(&rep_scores);
            let has_logged = c.window == 0;
            let m_log = min_margin(&log_scores);
            let pass = m_rep >= 0.0 && (!has_logged || m_log >= 0.0);
            let fmt_margin = |m: f32| {
                if m <= REJECT / 2.0 {
                    "REJECT".to_string()
                } else {
                    format!("{m:+.3}")
                }
            };
            let log_margin_cell = if has_logged {
                fmt_margin(m_log)
            } else {
                "-".into()
            };
            md.push_str(&format!(
                "| {} | {:.3} | {} / {} | {} | {nz} / {n_pairs} | {:.1} / {:.1} / {:.1} / {:.1} | {ge5} / {ge20} | {:.1} | {} / {} | {:.1} / {:.1} | {} / {} | {} / {} | {} / {} | {} | {:.1} |\n",
                c.name(),
                thr,
                pct(far_tot[0].0, far_tot[0].1),
                pct(far_tot[1].0, far_tot[1].1),
                pct(same_tot.0, same_tot.1),
                q(&bed_sorted, 0.5),
                q(&bed_sorted, 0.9),
                q(&bed_sorted, 0.99),
                q(&bed_sorted, 1.0),
                clean_max,
                pct(frr_tot[0].0, frr_tot[0].1),
                pct(frr_tot[1].0, frr_tot[1].1),
                q(&caller_frr[0], 0.9),
                q(&caller_frr[0], 1.0),
                over5(0),
                over5(1),
                pct(tfa, tfd),
                pct(tra, trd),
                fmt_margin(m_rep),
                log_margin_cell,
                if pass { "yes" } else { "**no**" },
                WINDOWS_S[c.window] - 1.5
            ));

            let mut worst = pair_far[0].clone();
            worst.sort_by(|x, y| y.0.total_cmp(&x.0));
            let clean_of = |a: usize, b: usize| {
                pair_far[1]
                    .iter()
                    .find(|p| p.1 == a && p.2 == b)
                    .map_or(0.0, |p| p.0)
            };
            let cells: Vec<String> = worst
                .iter()
                .take(10)
                .filter(|p| p.0 > 0.0)
                .map(|p| {
                    format!(
                        "{} ← {} {:.1} / {:.1}",
                        test_sp[p.1].label(),
                        test_sp[p.2].label(),
                        p.0 * 100.0,
                        clean_of(p.1, p.2) * 100.0
                    )
                })
                .collect();
            worst_md.push_str(&format!(
                "| {} | {} |\n",
                c.name(),
                if cells.is_empty() {
                    "none".to_string()
                } else {
                    cells.join("; ")
                }
            ));

            // Piper cross-check.
            let (pg, pi) = set_scores(&piper, c, cohort);
            let idx = |name: &str| piper_voices.iter().position(|v| pname(v) == name);
            let pair_cell = |a: &str, b: &str| match (idx(a), idx(b)) {
                (Some(a), Some(b)) => {
                    let bed = rate(&pi[0][a][b], thr, true);
                    let clean = rate(&pi[1][a][b], thr, true);
                    format!("{} / {}", pct(bed.0, bed.1), pct(clean.0, clean.1))
                }
                _ => "-".into(),
            };
            let mut ptot = [(0usize, 0usize); 2];
            let mut pworst = (0.0f64, String::new());
            for k in 0..2 {
                for a in 0..piper.len() {
                    for b in 0..piper.len() {
                        if a == b {
                            continue;
                        }
                        let (n, d) = rate(&pi[k][a][b], thr, true);
                        ptot[k].0 += n;
                        ptot[k].1 += d;
                        let f = n as f64 / d.max(1) as f64;
                        if k == 0 && f > pworst.0 {
                            pworst = (
                                f,
                                format!(
                                    "{} ← {}",
                                    pname(&piper_voices[a]),
                                    pname(&piper_voices[b])
                                ),
                            );
                        }
                    }
                }
            }
            let frr_cell = |name: &str| match idx(name) {
                Some(a) => {
                    let bed = rate(&pg[0][a], thr, false);
                    let clean = rate(&pg[1][a], thr, false);
                    format!("{} / {}", pct(bed.0, bed.1), pct(clean.0, clean.1))
                }
                None => "-".into(),
            };
            let mut other_max = [0.0f64; 2];
            for (a, v) in piper_voices.iter().enumerate() {
                if v.bundle.contains("libritts") {
                    continue;
                }
                for (k, m) in other_max.iter_mut().enumerate() {
                    let (n, d) = rate(&pg[k][a], thr, false);
                    *m = m.max(100.0 * n as f64 / d.max(1) as f64);
                }
            }
            piper_md.push_str(&format!(
                "| {} | {:.3} | {} / {} | {} | {} | {} | {} | {:.1} ({}) | {} | {} | {:.1} / {:.1} |\n",
                c.name(),
                thr,
                pct(ptot[0].0, ptot[0].1),
                pct(ptot[1].0, ptot[1].1),
                pair_cell("amy-low", "libritts_r 120"),
                pair_cell("libritts_r 120", "amy-low"),
                pair_cell("libritts_r 300", "lessac"),
                pair_cell("lessac", "hfc_female"),
                pworst.0 * 100.0,
                pworst.1,
                frr_cell("libritts_r 120"),
                frr_cell("libritts_r 300"),
                other_max[0],
                other_max[1],
            ));

            air_md.push_str(&format!(
                "| {} | {:.3} | {} | {} | {} | {} / {} |\n",
                c.name(),
                thr,
                fmt_list(&rep_scores[0]),
                fmt_list(&rep_scores[1]),
                if has_logged {
                    format!(
                        "{} / {}",
                        fmt_list(&log_scores[0]),
                        fmt_list(&log_scores[1])
                    )
                } else {
                    "-".into()
                },
                fmt_margin(m_rep),
                log_margin_cell,
            ));
        }
        md.push_str(&worst_md);
        md.push_str(&piper_md);
        md.push_str(&air_md);

        // Cohort statistics: how close each set sits to each cohort (TitaNet 1.5 s, bed).
        md.push_str("\n### Cohort statistics (TitaNet 1.5 s windows, bed / clean)\n\nMean over all windows of the set of the window-side cohort mean / std (`μt` / `σt`), full cohort.\n\n| set | real-speech cohort μt / σt | Piper cohort μt / σt |\n|---|---|---|\n");
        let stats = |set: &[[VoiceEmb; 2]], cohort: &Cohort, ci: usize| {
            let (mut sm, mut ss, mut n) = (0.0f32, 0.0f32, 0usize);
            for v in set {
                for w in &v[ci].windows[0][0] {
                    let (m, sd) = cohort_stats(w, &cohort[ci][0], None);
                    sm += m;
                    ss += sd;
                    n += 1;
                }
            }
            let n = n.max(1) as f32;
            format!("{:.3} / {:.3}", sm / n, ss / n)
        };
        for (label, set) in [
            ("LibriSpeech test-clean", &test),
            ("Piper test voices", &piper),
        ] {
            md.push_str(&format!(
                "| {label} | {} ; {} | {} ; {} |\n",
                stats(set, &ls_cohort, 0),
                stats(set, &ls_cohort, 1),
                stats(set, &p_cohort, 0),
                stats(set, &p_cohort, 1),
            ));
        }
        print!("{md}");
        if let Some(out) = &args.out {
            fs::write(out, &md).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
