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

use node_webrtc_rust_speech::speaker_gate::{
    ADAPT_MARGIN, ANCHOR_MIN_COSINE, CONFIDENT_OTHER_MAX_SCORE, NEAR_MISS_MIN_SCORE,
};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

#[path = "support/cand.rs"]
mod cand;
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
/// The product's `SpeakerGateConfig` default threshold (`config.rs`), TitaNet-small.
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
        &[],
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
    enroll_bounds: &[usize],
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
            if enroll_bounds.is_empty() {
                ve.enroll.push(emb.embed(&enroll[e0..e1], None)?);
            } else {
                // Product enrollment (`FA_ENROLL=product`): whole utterances, bed applied to their
                // concatenation; shared with near_miss_bench (`support/cand.rs`).
                let mut units: Vec<&[f32]> = Vec::new();
                let mut from = 0;
                for &to in enroll_bounds {
                    units.push(&enroll[from..to]);
                    from = to;
                }
                let c = cand::cand();
                let r = cand::enroll_product(
                    units.len(),
                    |i| Ok(Some(units[i].to_vec())),
                    |x| emb.embed(x, None).ok(),
                    &c,
                )?
                .ok_or("product enrollment: embed failed")?;
                ve.enroll.push(r.0);
            }
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
    /// `--models <dir>:<file,…>` (LibriSpeech mode): the model screen of plan
    /// `2026-11-17-speaker-model-screen` chunk 2, TitaNet-small from `--speaker-dir` as reference.
    models: Option<String>,
    /// `--judge dev-clean,dev-other`: held-out sets of the model screen.
    judge: String,
    /// `--map-constants <dir>:<file>` (LibriSpeech mode, `FA_ENROLL=product`): the gate constants
    /// of one model mapped from TitaNet-small's on the same-session tune pool (plan
    /// `2026-11-18-caller-loss-zhen-c7` decision a).
    map_constants: Option<String>,
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
        models: map.get("--models").cloned(),
        map_constants: map.get("--map-constants").cloned(),
        judge: map
            .get("--judge")
            .cloned()
            .unwrap_or_else(|| "dev-clean,dev-other".into()),
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

/// Model check (plan `2026-11-17-speaker-model-screen`, chunk 1): embedding dimension and
/// single-thread embed latency per model on the same real-speech windows.
/// `--model-check <models dir> --librispeech <LibriSpeech dir> [--windows 200]`; no other flag.
fn model_check(argv: &[String]) -> Result<(), String> {
    let arg = |k: &str| {
        argv.iter()
            .position(|a| a == k)
            .and_then(|i| argv.get(i + 1))
    };
    let dir = PathBuf::from(arg("--model-check").ok_or("--model-check needs a dir")?);
    let root = PathBuf::from(arg("--librispeech").ok_or("missing --librispeech")?);
    let n: usize = arg("--windows").map_or(Ok(200), |s| s.parse().map_err(|_| "bad --windows"))?;
    let sr = SR as usize;
    let (w15, w75) = (sr * (1500 + PRE_ROLL_MS) / 1000, sr * 750 / 1000);
    // Windows: one per test-clean utterance (sorted), cut at a fixed offset inside the utterance.
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.join("test-clean")];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "wav") {
                files.push(p);
            }
        }
    }
    files.sort();
    let step = (files.len() / n).max(1);
    let (mut a15, mut a75) = (Vec::new(), Vec::new());
    for f in files.iter().step_by(step) {
        let a = read_wav(f)?;
        if a.len() > sr + w15 {
            a15.push(a[sr..sr + w15].to_vec());
        }
        if a.len() > sr + w75 {
            a75.push(a[sr..sr + w75].to_vec());
        }
        if a15.len() >= n && a75.len() >= n {
            break;
        }
    }
    println!(
        "windows: {} x 1.8 s (1.5 s + 300 ms pre-roll), {} x 750 ms",
        a15.len(),
        a75.len()
    );
    println!("| model | dim | 1.5 s p50 / p99 ms | 750 ms p50 / p99 ms | 19 x 750 ms p99 ms | S3 scan <= 450 ms |");
    let mut models: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "onnx"))
        .collect();
    models.sort();
    for m in models {
        let mut emb = Embedder::open(&m)?;
        let mut dim = 0;
        for w in a15.iter().take(5) {
            dim = emb.embed(w, None)?.len(); // warm-up
        }
        for w in &a15 {
            emb.embed(w, Some(15))?;
        }
        for w in &a75 {
            emb.embed(w, Some(75))?;
        }
        let (mut t15, mut t75) = (emb.times[&15].clone(), emb.times[&75].clone());
        let (p50a, p99a) = (percentile(&mut t15, 0.5), percentile(&mut t15, 0.99));
        let (p50b, p99b) = (percentile(&mut t75, 0.5), percentile(&mut t75, 0.99));
        println!(
            "| {} | {dim} | {p50a:.1} / {p99a:.1} | {p50b:.1} / {p99b:.1} | {:.0} | {} |",
            emb.name,
            19.0 * p99b,
            if 19.0 * p99b <= 450.0 { "pass" } else { "FAIL" }
        );
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--model-check") {
        return model_check(&argv);
    }
    let args = parse_args()?;
    fs::create_dir_all(&args.cache).map_err(|e| e.to_string())?;
    if let Some(root) = &args.librispeech {
        if let Some(spec) = &args.map_constants {
            return librispeech::map_constants(&args, root, spec);
        }
        if let Some(spec) = &args.models {
            return librispeech::screen(&args, root, spec);
        }
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
/// CC BY 4.0) `test-clean` speakers are the callers and others; `dev-clean` speakers are split
/// per sex into the tune set and the impostor cohort (disjoint). TitaNet-small only, 1.5 s and
/// 2.0 s windows. Two modes: `cross-session` (enrollment from the first chapter, windows from the
/// other chapters) and `same-session` (enrollment = the first 3 s from the voiced onset of one
/// chapter, windows = the rest of that chapter, as a call enrolls from its own audio). The Piper
/// test voices and the airport demo are re-scored as cross-checks, the normalized candidates are
/// also run with the Piper cohort (can the cohort asset be synthetic?), and the `raw AND …` rules
/// are swept over thresholds above the tuned one.
mod librispeech {
    use super::*;

    /// Cross-session: enrollment source length (the 3000 ms clip is cut from its voiced onset).
    const ENROLL_SRC_S: usize = 5;
    const STREAM_S: usize = 120;
    /// `dev-clean` split per sex (it has 20 + 20 speakers): tune, then cohort.
    const TUNE_PER_SEX: usize = 5;
    const COHORT_PER_SEX: usize = 15;
    /// Tune-FRR steps above today's gate for the `raw AND …` sweep (percentage points).
    const SWEEP_PT: [f64; 4] = [1.0, 2.0, 3.0, 5.0];
    /// Margin the airport-limited sweep row keeps below the lowest logged-raw caller window.
    const AIRPORT_MARGIN: f32 = 0.02;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        CrossSession,
        SameSession,
    }

    impl Mode {
        fn name(self) -> &'static str {
            match self {
                Mode::CrossSession => "cross-session",
                Mode::SameSession => "same-session",
            }
        }
    }

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

    /// Where a speaker's enrollment and windows come from.
    struct Source {
        enroll: String,
        windows: String,
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

    fn stem(p: &Path) -> String {
        p.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn chapter(p: &Path) -> String {
        p.parent().map(stem).unwrap_or_default()
    }

    /// Utterances joined with 200 ms gaps until at least `max_s` seconds; (audio, files used).
    fn join<'a>(
        files: impl IntoIterator<Item = &'a PathBuf>,
        max_s: usize,
    ) -> Result<(Vec<f32>, Vec<&'a PathBuf>), String> {
        let mut out = Vec::new();
        let mut used = Vec::new();
        for f in files {
            if out.len() >= max_s * SR as usize {
                break;
            }
            out.extend(read_wav(f)?);
            out.extend(std::iter::repeat_n(0.0f32, GAP_MS * MS));
            used.push(f);
        }
        Ok((out, used))
    }

    fn span(files: &[&PathBuf]) -> String {
        match (files.first(), files.last()) {
            (Some(a), Some(b)) => format!("{} .. {}", stem(a), stem(b)),
            _ => "-".into(),
        }
    }

    type VoicedSrc = (Vec<f32>, Vec<f32>, Source, Vec<usize>);

    /// (clean enrollment source, clean talk stream, source), both at -20 dBFS.
    fn audio(sp: &Speaker, mode: Mode, product: bool) -> Result<VoicedSrc, String> {
        if product {
            return audio_product(sp, mode);
        }
        match mode {
            Mode::CrossSession => {
                let (enroll, used) = join(&sp.chapters[0], ENROLL_SRC_S)?;
                let rest: Vec<&PathBuf> = sp.chapters[1..]
                    .iter()
                    .flatten()
                    .chain(sp.chapters[0][used.len()..].iter())
                    .collect();
                let (stream, sused) = join(rest, STREAM_S)?;
                let mut chs: Vec<String> = sused.iter().map(|p| chapter(p)).collect();
                chs.dedup();
                Ok((
                    normalize_to(&enroll, MAIN_DB),
                    normalize_to(&stream, MAIN_DB),
                    Source {
                        enroll: format!("{} ({})", chapter(&sp.chapters[0][0]), span(&used)),
                        windows: chs.join(", "),
                    },
                    Vec::new(),
                ))
            }
            Mode::SameSession => {
                // The chapter with the most audio is the "call".
                let size = |ch: &Vec<PathBuf>| {
                    ch.iter()
                        .filter_map(|p| fs::metadata(p).ok())
                        .map(|m| m.len())
                        .sum::<u64>()
                };
                let ch = sp
                    .chapters
                    .iter()
                    .max_by_key(|c| size(c))
                    .ok_or("no chapter")?;
                let (raw, used) = join(ch, ENROLL_SRC_S + STREAM_S)?;
                let all = normalize_to(&raw, MAIN_DB);
                let onset = all
                    .chunks(20 * MS)
                    .position(|f| voiced_share(f) > 0.0)
                    .unwrap_or(0)
                    * 20
                    * MS;
                let cut = (onset + ENROLL_MS * MS).min(all.len());
                Ok((
                    all[..cut].to_vec(),
                    all[cut..].to_vec(),
                    Source {
                        enroll: format!("{} ({}, first 3 s)", chapter(&ch[0]), stem(&ch[0])),
                        windows: format!("{} (rest, {} files)", chapter(&ch[0]), used.len()),
                    },
                    Vec::new(),
                ))
            }
        }
    }

    /// Voiced part of `a`: first to last 20 ms frame above -45 dBFS (the bench's voiced test).
    fn trim_voiced(a: &[f32]) -> Option<&[f32]> {
        let fr = 20 * MS;
        let n = a.len() / fr;
        let on = (0..n).find(|i| voiced_share(&a[i * fr..(i + 1) * fr]) > 0.0)?;
        let off = (0..n)
            .rev()
            .find(|i| voiced_share(&a[i * fr..(i + 1) * fr]) > 0.0)?;
        Some(&a[on * fr..(off + 1) * fr])
    }

    /// Product enrollment audio (`FA_ENROLL=product`): whole utterances in file order, each from
    /// its voiced onset to its voiced end (no pre-roll, no gaps between utterances), appended until
    /// the enrollment span; the windows come from the files after them. Returns (enrollment,
    /// stream, source, cumulative end of each enrollment utterance in the enrollment). Same
    /// -20 dBFS normalization as the fixed path (over the joined audio of the speaker's source).
    fn audio_product(sp: &Speaker, mode: Mode) -> Result<VoicedSrc, String> {
        let span_ms = cand::cand().enroll_ms;
        let (files, stream_files): (Vec<&PathBuf>, Vec<&PathBuf>) = match mode {
            Mode::CrossSession => (
                sp.chapters[0].iter().collect(),
                sp.chapters[1..].iter().flatten().collect(),
            ),
            Mode::SameSession => {
                let size = |ch: &Vec<PathBuf>| {
                    ch.iter()
                        .filter_map(|p| fs::metadata(p).ok())
                        .map(|m| m.len())
                        .sum::<u64>()
                };
                let ch = sp
                    .chapters
                    .iter()
                    .max_by_key(|c| size(c))
                    .ok_or("no chapter")?;
                (ch.iter().collect(), Vec::new())
            }
        };
        let (raw, used) = join(
            files.iter().copied(),
            match mode {
                Mode::CrossSession => 40,
                Mode::SameSession => ENROLL_SRC_S + STREAM_S + 30,
            },
        )?;
        let all = normalize_to(&raw, MAIN_DB);
        let mut enroll: Vec<f32> = Vec::new();
        let mut bounds = Vec::new();
        let mut off = 0usize;
        let mut k = 0usize;
        for f in &used {
            let len = read_wav(f)?.len();
            if let Some(u) = trim_voiced(&all[off..off + len]) {
                enroll.extend_from_slice(u);
                bounds.push(enroll.len());
            }
            off += len + GAP_MS * MS;
            k += 1;
            if enroll.len() >= span_ms * MS {
                break;
            }
        }
        if enroll.len() < span_ms * MS {
            return Err(format!(
                "speaker {}: product enrollment needs {span_ms} ms, got {}",
                sp.id,
                enroll.len() / MS
            ));
        }
        let enroll_src = format!(
            "{} ({}, {} utterances, {} ms)",
            chapter(files[0]),
            span(&used[..k]),
            bounds.len(),
            enroll.len() / MS
        );
        let (stream, windows) = match mode {
            Mode::SameSession => {
                let start = off.min(all.len());
                let stream_end = (start + STREAM_S * SR as usize).min(all.len());
                (
                    all[start..stream_end].to_vec(),
                    format!("{} (rest, {} files)", chapter(files[0]), used.len() - k),
                )
            }
            Mode::CrossSession => {
                let rest: Vec<&PathBuf> = stream_files
                    .iter()
                    .copied()
                    .chain(files[k..].iter().copied())
                    .collect();
                let (st, sused) = join(rest, STREAM_S)?;
                let mut chs: Vec<String> = sused.iter().map(|p| chapter(p)).collect();
                chs.dedup();
                (normalize_to(&st, MAIN_DB), chs.join(", "))
            }
        };
        Ok((
            enroll,
            stream,
            Source {
                enroll: enroll_src,
                windows,
            },
            bounds,
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

    fn candidates() -> Vec<Cand> {
        let mut cands = Vec::new();
        for w in [0, 1] {
            cands.push(Cand {
                window: w,
                norm: Norm::Raw,
                fixed: Some(GATE_THRESHOLD),
                piper_cohort: false,
            });
        }
        for w in [0, 1] {
            cands.push(Cand {
                window: w,
                norm: Norm::Raw,
                fixed: None,
                piper_cohort: false,
            });
        }
        let norms = [
            Norm::Snorm,
            Norm::Asnorm,
            Norm::RawAndSnorm,
            Norm::RawAndAsnorm,
        ];
        for w in [0, 1] {
            for norm in norms {
                cands.push(Cand {
                    window: w,
                    norm,
                    fixed: None,
                    piper_cohort: false,
                });
            }
        }
        for norm in norms {
            cands.push(Cand {
                window: 0,
                norm,
                fixed: None,
                piper_cohort: true,
            });
        }
        cands
    }

    type Scores = (Vec<Vec<Vec<f32>>>, Vec<Vec<Vec<Vec<f32>>>>);

    /// genuine[cond][caller], impostor[cond][caller][other] for TitaNet (model 0).
    fn set_scores(set: &[[VoiceEmb; 2]], window: usize, norm: Norm, cohort: &Cohort) -> Scores {
        let mut gen = vec![vec![Vec::new(); set.len()]; 2];
        let mut imp = vec![vec![vec![Vec::new(); set.len()]; set.len()]; 2];
        for ci in 0..2 {
            for (a, va) in set.iter().enumerate() {
                let e = &va[ci].enroll[0];
                for (b, vb) in set.iter().enumerate() {
                    for w in &vb[ci].windows[0][window] {
                        let s = score(norm, e, w, &cohort[ci][0]);
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

    fn fmt_margin(m: f32) -> String {
        if m <= REJECT / 2.0 {
            "REJECT".to_string()
        } else {
            format!("{m:+.3}")
        }
    }

    /// One threshold on the test set: pooled rates, per-pair FAR and per-caller FRR.
    struct Eval {
        far: [(usize, usize); 2],
        same_sex: (usize, usize),
        /// [cond] -> (FAR, caller, other), same pair order in both conditions
        pairs: [Vec<(f64, usize, usize)>; 2],
        frr: [(usize, usize); 2],
        /// [cond] -> FRR % per caller
        caller_frr: [Vec<f64>; 2],
    }

    impl Eval {
        fn new(
            gen: &[Vec<Vec<f32>>],
            imp: &[Vec<Vec<Vec<f32>>>],
            thr: f32,
            sp: &[Speaker],
        ) -> Self {
            let n = sp.len();
            let mut e = Eval {
                far: [(0, 0); 2],
                same_sex: (0, 0),
                pairs: [Vec::new(), Vec::new()],
                frr: [(0, 0); 2],
                caller_frr: [Vec::new(), Vec::new()],
            };
            for k in 0..2 {
                for a in 0..n {
                    for b in 0..n {
                        if a == b {
                            continue;
                        }
                        let (x, d) = rate(&imp[k][a][b], thr, true);
                        e.far[k].0 += x;
                        e.far[k].1 += d;
                        if k == 0 && sp[a].female == sp[b].female {
                            e.same_sex.0 += x;
                            e.same_sex.1 += d;
                        }
                        e.pairs[k].push((x as f64 / d.max(1) as f64, a, b));
                    }
                    let (x, d) = rate(&gen[k][a], thr, false);
                    e.frr[k].0 += x;
                    e.frr[k].1 += d;
                    e.caller_frr[k].push(100.0 * x as f64 / d.max(1) as f64);
                }
            }
            e
        }

        fn bed_sorted(&self) -> Vec<f64> {
            let mut v: Vec<f64> = self.pairs[0].iter().map(|p| p.0 * 100.0).collect();
            v.sort_by(f64::total_cmp);
            v
        }

        fn pairs_ge(&self, p: f64) -> usize {
            self.pairs[0].iter().filter(|x| x.0 * 100.0 >= p).count()
        }

        /// The `n` worst pairs (bed) as "caller ← other bed / clean".
        fn worst(&self, n: usize, sp: &[Speaker]) -> Vec<String> {
            let mut idx: Vec<usize> = (0..self.pairs[0].len()).collect();
            idx.sort_by(|x, y| self.pairs[0][*y].0.total_cmp(&self.pairs[0][*x].0));
            idx.into_iter()
                .take(n)
                .filter(|i| self.pairs[0][*i].0 > 0.0)
                .map(|i| {
                    let (f, a, b) = self.pairs[0][i];
                    format!(
                        "{} ← {} {:.1} / {:.1}",
                        sp[a].label(),
                        sp[b].label(),
                        f * 100.0,
                        self.pairs[1][i].0 * 100.0
                    )
                })
                .collect()
        }

        fn sorted_frr(&self, k: usize) -> Vec<f64> {
            let mut v = self.caller_frr[k].clone();
            v.sort_by(f64::total_cmp);
            v
        }
    }

    /// Airport caller windows scored by one rule: reproduced and at the logged raw cosine.
    struct AirScores {
        rep: Vec<Vec<f32>>,
        logged: Vec<Vec<f32>>,
        has_logged: bool,
    }

    impl AirScores {
        fn new(
            models: &mut [Embedder],
            airport: &[AirportBase],
            window: usize,
            norm: Norm,
            cohort: &Cohort,
        ) -> Result<Self, String> {
            let mut rep = Vec::new();
            let mut logged = Vec::new();
            for (ap, base) in AIRPORT.iter().zip(airport) {
                let mut row = Vec::new();
                let mut lrow = Vec::new();
                for &k in ap.caller {
                    let end = base.u_ms + k * 1500;
                    let w = (WINDOWS_S[window] * 1000.0) as usize;
                    let e = models[0].embed(clip(&base.audio, end - w - PRE_ROLL_MS, end), None)?;
                    row.push(score(norm, &base.refs[0], &e, &cohort[0][0]));
                    lrow.push(score_from_raw(
                        norm,
                        ap.logged[k - 1],
                        &base.refs[0],
                        &e,
                        &cohort[0][0],
                    ));
                }
                rep.push(row);
                logged.push(lrow);
            }
            Ok(Self {
                rep,
                logged,
                has_logged: window == 0,
            })
        }

        fn margin(v: &[Vec<f32>], thr: f32) -> f32 {
            v.iter()
                .flatten()
                .map(|s| s - thr)
                .fold(f32::INFINITY, f32::min)
        }

        fn pass(&self, thr: f32) -> bool {
            Self::margin(&self.rep, thr) >= 0.0
                && (!self.has_logged || Self::margin(&self.logged, thr) >= 0.0)
        }

        fn margins(&self, thr: f32) -> String {
            format!(
                "{} / {}",
                fmt_margin(Self::margin(&self.rep, thr)),
                if self.has_logged {
                    fmt_margin(Self::margin(&self.logged, thr))
                } else {
                    "-".into()
                }
            )
        }

        fn min_logged(&self) -> f32 {
            self.logged
                .iter()
                .flatten()
                .copied()
                .fold(f32::INFINITY, f32::min)
        }
    }

    /// Everything both modes share.
    struct Shared {
        ls_cohort: Cohort,
        p_cohort: Cohort,
        piper: Vec<[VoiceEmb; 2]>,
        piper_voices: Vec<Voice>,
        airport: Vec<AirportBase>,
    }

    fn pname(v: &Voice) -> String {
        let short = v
            .name
            .trim_start_matches("en_US-")
            .trim_end_matches("-medium");
        if v.bundle.contains("libritts") {
            format!("{short} {}", v.sid)
        } else {
            short.to_string()
        }
    }

    /// Smallest threshold above `x` (so `x` itself is rejected).
    fn above(x: f32) -> f32 {
        x + x.abs() * 1e-6 + 1e-6
    }

    /// Per-mode enrollment sentence of the report header.
    fn mode_enroll_text(mode: Mode, product: bool) -> &'static str {
        match (mode, product) {
            (Mode::CrossSession, false) => {
                "Enrollment: 3000 ms from the voiced onset of the first chapter's utterances; \
                 windows from the other chapters. "
            }
            (Mode::SameSession, false) => {
                "Enrollment: the first 3000 ms from the voiced onset of the speaker's longest \
                 chapter; windows: the rest of that chapter after the enrollment span (no \
                 overlap). "
            }
            (Mode::CrossSession, true) => {
                "Enrollment (product): whole voiced utterances of the first chapter until 3000 ms; \
                 windows from the other chapters, then the rest of the first chapter. "
            }
            (Mode::SameSession, true) => {
                "Enrollment (product): whole voiced utterances of the speaker's longest chapter in \
                 file order until 3000 ms, one embed; windows: up to 120 s from the next file (no \
                 overlap). "
            }
        }
    }

    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    fn report(
        mode: Mode,
        models: &mut [Embedder],
        sh: &Shared,
        test: &[[VoiceEmb; 2]],
        tune: &[[VoiceEmb; 2]],
        test_sp: &[Speaker],
        src: &[Source],
        stream_s: f64,
        product: bool,
    ) -> Result<String, String> {
        let m = mode.name();
        let today_tune = set_scores(tune, 0, Norm::Raw, &sh.ls_cohort);
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
        let mut md = format!(
            "\n### Mode: {m}\n\n{}Talk stream mean {stream_s:.0} s per speaker. Windows per speaker \
             and condition: mean {w15:.0} (min {w15min}) at 1.5 s, {w20:.0} (min {w20min}) at 2.0 s; \
             {n_pairs} pairs. **Tune FRR target ({m}): {:.1} %** (today's gate's tune FRR {} %, \
             floor 1 %).\n\n",
            mode_enroll_text(mode, product),
            target * 100.0,
            pct(today_tune_frr.0, today_tune_frr.1),
        );

        md.push_str(&format!("#### Real speech ({m}): summary per candidate\n\nPer-pair FAR distribution over the pairs (bed); FRR distribution over the 40 callers.\n\n| candidate | threshold | test FAR bed / clean % | same-sex FAR bed % | pairs with FAR > 0, bed | pair FAR p50 / p90 / p99 / max bed % | pairs >= 5 % / >= 20 % bed | pair FAR max clean % | test FRR bed / clean % | caller FRR p90 / max bed % | callers FRR > 5 % bed / clean | tune FAR / FRR % | airport min margin reproduced / logged raw | airport pass | extra confirm delay s |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n"));
        let mut worst_md = format!("\n#### Real speech ({m}): worst 10 pairs per candidate (caller ← other, bed / clean %)\n\n| candidate | worst pairs |\n|---|---|\n");
        let mut piper_md = String::from("\n#### Cross-check: Piper test voices scored with the real-speech thresholds and cohort (cross-session tune)\n\nSame windows as chunk 1b (bed / clean %).\n\n| candidate | threshold | Piper FAR bed / clean | amy-low ← libritts_r 120 | libritts_r 120 ← amy-low | libritts_r 300 ← lessac | lessac ← hfc_female | worst pair bed | FRR libritts_r 120 | FRR libritts_r 300 | max FRR other callers bed / clean |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
        let mut air_md = String::from("\n#### Cross-check: airport demo caller windows (cross-session tune)\n\nReproduced offline (enrollment and window embeddings) and at the logged raw cosine (1.5 s only; cohort statistics from the reproduced embeddings). Cohort as in the candidate; bed condition cohort.\n\n| candidate | threshold | A w9, w10 reproduced | B w5, w6, w7 reproduced | A / B at logged raw | min margin reproduced / logged |\n|---|---|---|---|---|---|\n");

        for c in &candidates() {
            let cohort = if c.piper_cohort {
                &sh.p_cohort
            } else {
                &sh.ls_cohort
            };
            let (tg, ti) = set_scores(tune, c.window, c.norm, cohort);
            let thr = c.fixed.unwrap_or_else(|| tuned_threshold(&tg, target));
            let (gen, imp) = set_scores(test, c.window, c.norm, cohort);
            let ev = Eval::new(&gen, &imp, thr, test_sp);
            let bed_sorted = ev.bed_sorted();
            let clean_max = ev.pairs[1].iter().map(|p| p.0 * 100.0).fold(0.0, f64::max);
            let nz = bed_sorted.iter().filter(|x| **x > 0.0).count();
            let over5 = |k: usize| ev.caller_frr[k].iter().filter(|x| **x > 5.0).count();
            let frr_bed = ev.sorted_frr(0);
            let tune_far: Vec<f32> = ti.iter().flatten().flatten().flatten().copied().collect();
            let tune_gen: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
            let (tfa, tfd) = rate(&tune_far, thr, true);
            let (tra, trd) = rate(&tune_gen, thr, false);
            let air = AirScores::new(models, &sh.airport, c.window, c.norm, cohort)?;
            md.push_str(&format!(
                "| {} | {:.3} | {} / {} | {} | {nz} / {n_pairs} | {:.1} / {:.1} / {:.1} / {:.1} | {} / {} | {:.1} | {} / {} | {:.1} / {:.1} | {} / {} | {} / {} | {} | {} | {:.1} |\n",
                c.name(),
                thr,
                pct(ev.far[0].0, ev.far[0].1),
                pct(ev.far[1].0, ev.far[1].1),
                pct(ev.same_sex.0, ev.same_sex.1),
                q(&bed_sorted, 0.5),
                q(&bed_sorted, 0.9),
                q(&bed_sorted, 0.99),
                q(&bed_sorted, 1.0),
                ev.pairs_ge(5.0),
                ev.pairs_ge(20.0),
                clean_max,
                pct(ev.frr[0].0, ev.frr[0].1),
                pct(ev.frr[1].0, ev.frr[1].1),
                q(&frr_bed, 0.9),
                q(&frr_bed, 1.0),
                over5(0),
                over5(1),
                pct(tfa, tfd),
                pct(tra, trd),
                air.margins(thr),
                if air.pass(thr) { "yes" } else { "**no**" },
                WINDOWS_S[c.window] - 1.5
            ));
            if mode != Mode::CrossSession {
                continue;
            }
            let cells = ev.worst(10, test_sp);
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
            let (pg, pi) = set_scores(&sh.piper, c.window, c.norm, cohort);
            let idx = |name: &str| sh.piper_voices.iter().position(|v| pname(v) == name);
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
                for (a, row) in pi[k].iter().enumerate() {
                    for (b, v) in row.iter().enumerate() {
                        if a == b {
                            continue;
                        }
                        let (n, d) = rate(v, thr, true);
                        ptot[k].0 += n;
                        ptot[k].1 += d;
                        let f = n as f64 / d.max(1) as f64;
                        if k == 0 && f > pworst.0 {
                            pworst = (
                                f,
                                format!(
                                    "{} ← {}",
                                    pname(&sh.piper_voices[a]),
                                    pname(&sh.piper_voices[b])
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
            for (a, v) in sh.piper_voices.iter().enumerate() {
                if v.bundle.contains("libritts") {
                    continue;
                }
                for (k, mx) in other_max.iter_mut().enumerate() {
                    let (n, d) = rate(&pg[k][a], thr, false);
                    *mx = mx.max(100.0 * n as f64 / d.max(1) as f64);
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
                "| {} | {:.3} | {} | {} | {} | {} |\n",
                c.name(),
                thr,
                fmt_list(&air.rep[0]),
                fmt_list(&air.rep[1]),
                if air.has_logged {
                    format!(
                        "{} / {}",
                        fmt_list(&air.logged[0]),
                        fmt_list(&air.logged[1])
                    )
                } else {
                    "-".into()
                },
                air.margins(thr),
            ));
        }
        if mode == Mode::CrossSession {
            md.push_str(&worst_md);
            md.push_str(&piper_md);
            md.push_str(&air_md);
        }

        // Today's gate on the test set, the reference for the sweep and the equal-FAR rows.
        let (tgen, timp) = set_scores(test, 0, Norm::Raw, &sh.ls_cohort);
        let tev = Eval::new(&tgen, &timp, GATE_THRESHOLD, test_sp);
        let tair = AirScores::new(models, &sh.airport, 0, Norm::Raw, &sh.ls_cohort)?;
        let row = |rule: &str,
                   label: &str,
                   thr: f32,
                   tune_frr: String,
                   ev: &Eval,
                   air: &AirScores| {
            format!(
                "| {rule} | {label} | {thr:.3} | {tune_frr} | {} / {} | {} / {} | {} | {} / {} | {} |\n",
                pct(ev.far[0].0, ev.far[0].1),
                pct(ev.far[1].0, ev.far[1].1),
                ev.pairs_ge(5.0),
                ev.pairs_ge(20.0),
                ev.worst(1, test_sp).first().cloned().unwrap_or("none".into()),
                pct(ev.frr[0].0, ev.frr[0].1),
                pct(ev.frr[1].0, ev.frr[1].1),
                air.margins(thr),
            )
        };
        md.push_str(&format!("\n#### Real speech ({m}): AND threshold sweep (1.5 s, real cohort)\n\nThe tuned row uses the tune FRR target above; with that target the normalized part cannot reject a genuine window the raw part accepts, so it equals today's gate by construction. The `+N pt` rows allow N points more tune FRR; the airport row is the highest threshold that keeps every airport caller window {AIRPORT_MARGIN} above it at the logged raw scores.\n\n| rule | row | threshold | tune FRR % | test FAR bed / clean % | pairs >= 5 % / >= 20 % bed | worst pair bed / clean % | test FRR bed / clean % | airport margin reproduced / logged raw |\n|---|---|---|---|---|---|---|---|---|\n"));
        md.push_str(&row(
            "raw (today)",
            "@0.459",
            GATE_THRESHOLD,
            pct(today_tune_frr.0, today_tune_frr.1),
            &tev,
            &tair,
        ));
        for norm in [Norm::RawAndAsnorm, Norm::RawAndSnorm] {
            let (tg, _) = set_scores(tune, 0, norm, &sh.ls_cohort);
            let tune_gen: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
            let (gen, imp) = set_scores(test, 0, norm, &sh.ls_cohort);
            let air = AirScores::new(models, &sh.airport, 0, norm, &sh.ls_cohort)?;
            let mut rows = vec![(
                "tuned (today's tune FRR)".to_string(),
                tuned_threshold(&tg, target),
            )];
            for pt in SWEEP_PT {
                rows.push((
                    format!("tune FRR +{pt:.0} pt"),
                    tuned_threshold(&tg, target + pt / 100.0),
                ));
            }
            rows.push((
                format!("airport limit (logged min - {AIRPORT_MARGIN})"),
                air.min_logged() - AIRPORT_MARGIN,
            ));
            for (label, thr) in rows {
                let ev = Eval::new(&gen, &imp, thr, test_sp);
                let (tr, td) = rate(&tune_gen, thr, false);
                md.push_str(&row(norm.name(), &label, thr, pct(tr, td), &ev, &air));
            }
        }

        md.push_str(&format!("\n#### Real speech ({m}): normalized scores alone at today's FAR (1.5 s, real cohort)\n\nThreshold set so the pooled test FAR (bed) is at most today's gate's ({} %).\n\n| rule | row | threshold | tune FRR % | test FAR bed / clean % | pairs >= 5 % / >= 20 % bed | worst pair bed / clean % | test FRR bed / clean % | airport margin reproduced / logged raw |\n|---|---|---|---|---|---|---|---|---|\n", pct(tev.far[0].0, tev.far[0].1)));
        for norm in [Norm::Snorm, Norm::Asnorm] {
            let (tg, _) = set_scores(tune, 0, norm, &sh.ls_cohort);
            let tune_gen: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
            let (gen, imp) = set_scores(test, 0, norm, &sh.ls_cohort);
            let mut bed_imp: Vec<f32> = imp[0].iter().flatten().flatten().copied().collect();
            bed_imp.sort_by(|a, b| b.total_cmp(a));
            let n0 = tev.far[0].0;
            let thr = bed_imp.get(n0).map_or(f32::MIN, |x| above(*x));
            let ev = Eval::new(&gen, &imp, thr, test_sp);
            let air = AirScores::new(models, &sh.airport, 0, norm, &sh.ls_cohort)?;
            let (tr, td) = rate(&tune_gen, thr, false);
            md.push_str(&row(norm.name(), "equal FAR", thr, pct(tr, td), &ev, &air));
        }

        md.push_str(&format!("\n#### Real speech ({m}): callers with FRR > 5 % (today's gate, bed or clean)\n\n| caller | enrollment (chapter, files) | window chapters | windows bed | FRR bed / clean % | genuine raw p10 / p50 bed | genuine raw p10 / p50 clean |\n|---|---|---|---|---|---|---|\n"));
        let mut any = false;
        for (a, sp) in test_sp.iter().enumerate() {
            if tev.caller_frr[0][a] <= 5.0 && tev.caller_frr[1][a] <= 5.0 {
                continue;
            }
            any = true;
            let pq = |k: usize| {
                let mut v: Vec<f64> = tgen[k][a].iter().map(|x| f64::from(*x)).collect();
                v.sort_by(f64::total_cmp);
                format!("{:.3} / {:.3}", q(&v, 0.1), q(&v, 0.5))
            };
            md.push_str(&format!(
                "| {} | {} | {} | {} | {:.1} / {:.1} | {} | {} |\n",
                sp.label(),
                src[a].enroll,
                src[a].windows,
                tgen[0][a].len(),
                tev.caller_frr[0][a],
                tev.caller_frr[1][a],
                pq(0),
                pq(1),
            ));
        }
        if !any {
            md.push_str("| none | | | | | | |\n");
        }

        if mode == Mode::CrossSession {
            md.push_str("\n#### Cohort statistics (TitaNet 1.5 s windows, bed ; clean)\n\nMean over all windows of the set of the window-side cohort mean / std (`μt` / `σt`), full cohort.\n\n| set | real-speech cohort μt / σt | Piper cohort μt / σt |\n|---|---|---|\n");
            let stats = |set: &[[VoiceEmb; 2]], cohort: &Cohort, ci: usize| {
                let (mut sm, mut ss, mut n) = (0.0f32, 0.0f32, 0usize);
                for v in set {
                    for w in &v[ci].windows[0][0] {
                        let (mm, sd) = cohort_stats(w, &cohort[ci][0], None);
                        sm += mm;
                        ss += sd;
                        n += 1;
                    }
                }
                let n = n.max(1) as f32;
                format!("{:.3} / {:.3}", sm / n, ss / n)
            };
            for (label, set) in [
                ("LibriSpeech test-clean", test),
                ("Piper test voices", sh.piper.as_slice()),
            ] {
                md.push_str(&format!(
                    "| {label} | {} ; {} | {} ; {} |\n",
                    stats(set, &sh.ls_cohort, 0),
                    stats(set, &sh.ls_cohort, 1),
                    stats(set, &sh.p_cohort, 0),
                    stats(set, &sh.p_cohort, 1),
                ));
            }
        }
        Ok(md)
    }

    pub(super) fn run(args: &Args, root: &Path) -> Result<(), String> {
        let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
        let mut models = vec![Embedder::open(
            &args.speaker_dir.join("nemo_en_titanet_small.onnx"),
        )?];
        // `FA_ENROLL=product`: product enrollment (whole utterances until `NM_CAND` span, one
        // embed; `c2`: mean of per-utterance embeddings). Default: today's fixed 3000 ms from the
        // voiced onset (reproduces the published rows). C4 and C7 do not change per-window scores:
        // their window FAR is that of the enrollment they sit on, so they are not re-run here.
        let product = std::env::var("FA_ENROLL").is_ok_and(|v| v == "product");
        let c = cand::cand();
        if c.c3 || c.c4 > 0 || c.c7_nm || c.c7_scan || c.c7_run != 2 {
            eprintln!(
                "false_accept_bench: NM_CAND tokens other than e<ms>/c2 do not change per-window scores; ignored"
            );
        }
        if (c.c2 || c.enroll_ms != cand::ENROLL_MS_DEFAULT) && !product {
            return Err("NM_CAND c2 / e<ms> need FA_ENROLL=product".into());
        }
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

        let ls_clips = cohort_sp
            .iter()
            .map(|sp| audio(sp, Mode::CrossSession, false).map(|(e, _, _, _)| e))
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
        let mut airport = Vec::new();
        for (ai, ap) in AIRPORT.iter().enumerate() {
            airport.push(fit_airport(&mut models, ap, &args.airport[ai])?);
        }
        let sh = Shared {
            ls_cohort,
            p_cohort,
            piper,
            piper_voices,
            airport,
        };

        let mut md = format!(
            "Enrollment: {} (`FA_ENROLL`={}, `NM_CAND`={}).\n\n",
            if product {
                "product (whole utterances appended until the span, one embed; c2: mean of per-utterance embeddings)"
            } else {
                "fixed 3000 ms from the voiced onset (published rows)"
            },
            if product { "product" } else { "fixed" },
            std::env::var("NM_CAND").unwrap_or_default()
        );
        md.push_str(&format!(
            "Test: LibriSpeech `test-clean`, {} speakers ({} F / {} M), each caller and other. Tune: \
             `dev-clean` {} speakers ({}). Cohort: `dev-clean` {} other speakers ({}), disjoint from \
             tune and test; cohort clips are 3.3 s from each cohort speaker's first chapter. Bed: \
             Tocumen airport at {} dB (`bed`) or none (`clean`). Thresholds: `@0.459` fixed, else \
             tune-matched: highest threshold with tune FRR <= the mode's target (today's gate's tune \
             FRR, floor 1 %).\n",
            test_sp.len(),
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
            args.bed_db,
        ));
        for mode in [Mode::CrossSession, Mode::SameSession] {
            let mut test = Vec::new();
            let mut src = Vec::new();
            let mut stream_s = 0.0;
            for (i, sp) in test_sp.iter().enumerate() {
                eprintln!("{} test speaker {}", mode.name(), sp.label());
                let (e, s, so, eb) = audio(sp, mode, product)?;
                stream_s += s.len() as f64 / f64::from(SR);
                src.push(so);
                test.push(embed_audio(
                    &mut models,
                    &eb,
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
                eprintln!("{} tune speaker {}", mode.name(), sp.label());
                let (e, s, _, eb) = audio(sp, mode, product)?;
                tune.push(embed_audio(
                    &mut models,
                    &eb,
                    &e,
                    &s,
                    60 + i,
                    &bed,
                    args.bed_db,
                    windows_15_20,
                )?);
            }
            md.push_str(&report(
                mode,
                &mut models,
                &sh,
                &test,
                &tune,
                &test_sp,
                &src,
                stream_s / test_sp.len() as f64,
                product,
            )?);
        }
        let mut times0 = models[0].times.get(&0).cloned().unwrap_or_default();
        let mut times1 = models[0].times.get(&1).cloned().unwrap_or_default();
        md.push_str(&format!(
            "\nTitaNet embed p50 / p99: {:.1} / {:.1} ms (1.5 s), {:.1} / {:.1} ms (2.0 s).\n",
            percentile(&mut times0, 0.5),
            percentile(&mut times0, 0.99),
            percentile(&mut times1, 0.5),
            percentile(&mut times1, 0.99),
        ));
        print!("{md}");
        if let Some(out) = &args.out {
            fs::write(out, &md).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Model screen (plan `2026-11-17-speaker-model-screen` §5): 1.5 s windows only, raw cosine.
    fn windows_15(_model: usize) -> &'static [usize] {
        &[0]
    }

    /// Per-speaker pooled scores of model `m`: genuine[cond][caller], impostor[cond][caller][other].
    fn raw_scores(set: &[[VoiceEmb; 2]], m: usize) -> Scores {
        let n = set.len();
        let mut gen = vec![vec![Vec::new(); n]; 2];
        let mut imp = vec![vec![vec![Vec::new(); n]; n]; 2];
        for ci in 0..2 {
            for (a, va) in set.iter().enumerate() {
                let e = &va[ci].enroll[m];
                for (b, vb) in set.iter().enumerate() {
                    for w in &vb[ci].windows[m][0] {
                        let s = cosine(e, w);
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

    /// One model, one held-out set, one mode, one condition.
    struct Cell {
        frr: (usize, usize),
        far: (usize, usize),
        ge5: usize,
        ge20: Vec<(usize, usize)>,
    }

    fn cell(set: &[[VoiceEmb; 2]], m: usize, thr: f32, k: usize, sp: &[Speaker]) -> Cell {
        let (gen, imp) = raw_scores(set, m);
        let ev = Eval::new(&gen, &imp, thr, sp);
        Cell {
            frr: ev.frr[k],
            far: ev.far[k],
            ge5: ev.pairs[k].iter().filter(|p| p.0 * 100.0 >= 5.0).count(),
            ge20: ev.pairs[k]
                .iter()
                .filter(|p| p.0 * 100.0 >= 20.0)
                .map(|p| (p.1, p.2))
                .collect(),
        }
    }

    fn rate_pct(r: (usize, usize)) -> f64 {
        100.0 * r.0 as f64 / r.1.max(1) as f64
    }

    /// Lowest threshold whose pooled tune FAR is at most the reference's `n0` accepts: the
    /// loosest threshold that matches the reference's FAR (a higher one only rejects more callers).
    fn far_matched(tune_imp_k: &[Vec<Vec<f32>>], n0: usize) -> f32 {
        let mut v: Vec<f32> = tune_imp_k.iter().flatten().flatten().copied().collect();
        v.sort_by(|a, b| b.total_cmp(a));
        v.get(n0).map_or(f32::MIN, |x| above(*x))
    }

    /// The lowest score at which at most `n` of `scores` are at or above it (`far_matched` on a
    /// flat set).
    fn survival_matched(scores: &[f32], n: usize) -> f32 {
        far_matched(&[vec![scores.to_vec()]], n)
    }

    /// Impostor-side mapping of `c`: the reference survivor count `n` (scores at or above `c`) and
    /// the target's FAR-matched score for it. `None` when the reference has no survivor (nothing to
    /// match).
    fn map_imp(ref_k: &[Vec<Vec<f32>>], tgt_k: &[Vec<Vec<f32>>], c: f32) -> Option<(usize, f32)> {
        let n = ref_k
            .iter()
            .flatten()
            .flatten()
            .filter(|x| **x >= c)
            .count();
        (n > 0).then(|| (n, far_matched(tgt_k, n)))
    }

    /// Genuine-side mapping of `c`: the reference survivor count and the target score with the same
    /// count of survivors.
    fn map_gen(ref_: &[f32], tgt: &[f32], c: f32) -> (usize, f32) {
        let n = ref_.iter().filter(|x| **x >= c).count();
        (n, survival_matched(tgt, n))
    }

    /// Constant mapping (plan `2026-11-18-caller-loss-zhen-c7`, decision a): TitaNet-small's gate
    /// constants mapped to one other model on the same-session tune pool (`test-clean`, product
    /// enrollment, 1.5 s windows, bed and clean). Impostor-side constants (threshold, near-miss
    /// floor, confident-other) are FAR-matched per condition with `far_matched` and the higher of
    /// the two is taken; a constant with no TitaNet survivor in a condition is an error (nothing to
    /// match). Threshold + margin and the anchor bound map by genuine-window survival, pooled over
    /// both conditions.
    pub(super) fn map_constants(args: &Args, root: &Path, spec: &str) -> Result<(), String> {
        if !std::env::var("FA_ENROLL").is_ok_and(|v| v == "product") {
            return Err("--map-constants needs FA_ENROLL=product".into());
        }
        let (dir, file) = spec
            .rsplit_once(':')
            .ok_or("--map-constants needs <dir>:<file>")?;
        let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
        let mut models = vec![
            Embedder::open(&args.speaker_dir.join("nemo_en_titanet_small.onnx"))?,
            Embedder::open(&Path::new(dir).join(file))?,
        ];
        let tune_sp = speakers(root, "test-clean")?;
        let mode = Mode::SameSession;
        let mut set = Vec::new();
        for (i, sp) in tune_sp.iter().enumerate() {
            eprintln!("map-constants speaker {}", sp.label());
            let (e, s, _, eb) = audio(sp, mode, true)?;
            set.push(embed_audio(
                &mut models,
                &eb,
                &e,
                &s,
                i,
                &bed,
                args.bed_db,
                windows_15,
            )?);
        }
        let sc: Vec<Scores> = (0..2).map(|m| raw_scores(&set, m)).collect();
        let flat =
            |v: &[Vec<Vec<f32>>]| -> Vec<f32> { v.iter().flatten().flatten().copied().collect() };
        let gflat = |v: &Vec<Vec<f32>>| -> Vec<f32> { v.iter().flatten().copied().collect() };
        let n_imp: Vec<usize> = (0..2).map(|k| flat(&sc[0].1[k]).len()).collect();
        let mut md = format!(
            "### Constant mapping on the false_accept_bench pool (decision a)\n\nTitaNet-small -> `{file}`. LibriSpeech test-clean, {} speakers, same-session, product enrollment, 1.5 s windows, raw cosine, bed {} dB. Impostor windows per condition: bed {}, clean {} (target the same). Genuine windows: {} (bed + clean).\n\n| constant | TitaNet value | TitaNet survivors (bed / clean) | mapped bed | mapped clean | mapped (higher) |\n| --- | --- | --- | --- | --- | --- |\n",
            tune_sp.len(), args.bed_db, n_imp[0], n_imp[1],
            gflat(&sc[0].0[0]).len() + gflat(&sc[0].0[1]).len(),
        );
        let mut mapped = Vec::new();
        for (name, c) in [
            ("threshold", GATE_THRESHOLD),
            ("near-miss floor", NEAR_MISS_MIN_SCORE),
            ("confident-other", CONFIDENT_OTHER_MAX_SCORE),
        ] {
            let mm: Vec<Option<(usize, f32)>> = (0..2)
                .map(|k| map_imp(&sc[0].1[k], &sc[1].1[k], c))
                .collect();
            let (Some((n0, m0)), Some((n1, m1))) = (mm[0], mm[1]) else {
                return Err(format!(
                    "{name} {c}: zero TitaNet survivors in a condition, nothing to match (stop and report)"
                ));
            };
            let (n, m) = ([n0, n1], [m0, m1]);
            let hi = m[0].max(m[1]);
            md.push_str(&format!(
                "| {name} | {c:.3} | {} / {} | {:.4} | {:.4} | {hi:.4} |\n",
                n[0], n[1], m[0], m[1]
            ));
            mapped.push(hi);
        }
        let gen_ref: Vec<f32> = (0..2).flat_map(|k| gflat(&sc[0].0[k])).collect();
        let gen_tgt: Vec<f32> = (0..2).flat_map(|k| gflat(&sc[1].0[k])).collect();
        let mut gmap = |name: &str, c: f32| -> f32 {
            let (n, m) = map_gen(&gen_ref, &gen_tgt, c);
            md.push_str(&format!(
                "| {name} | {c:.3} | {n} of {} genuine | {m:.4} | | {m:.4} |\n",
                gen_ref.len()
            ));
            m
        };
        let top = gmap(
            "threshold + margin (genuine survival)",
            GATE_THRESHOLD + ADAPT_MARGIN,
        );
        let anchor = gmap("anchor bound (genuine quantile)", ANCHOR_MIN_COSINE);
        let margin = (top - mapped[0]).max(0.0);
        md.push_str(&format!(
            "\nMapped margin: {top:.4} - {:.4} = {:.4} (floored at 0: {margin:.4}).\n\nFlags: `--thr {:.4} --nm-floor {:.4} --confident-other {:.4} --adapt-margin {margin:.4} --anchor-min {anchor:.4}`\n",
            mapped[0], top - mapped[0], mapped[0], mapped[1], mapped[2]
        ));
        print!("{md}");
        if let Some(out) = &args.out {
            fs::write(out, &md).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Equal error rate of raw scores: at the candidate threshold (every score, and above them all)
    /// where FRR (genuine < thr) and FAR (impostor >= thr) are closest, their mean.
    fn eer(gen: &[f32], imp: &[f32]) -> f64 {
        if gen.is_empty() || imp.is_empty() {
            return 0.0;
        }
        let mut cands: Vec<f32> = gen.iter().chain(imp).copied().collect();
        cands.push(f32::INFINITY);
        let mut best = (f64::INFINITY, 0.0);
        for t in cands {
            let frr = rate(gen, t, false).0 as f64 / gen.len() as f64;
            let far = rate(imp, t, true).0 as f64 / imp.len() as f64;
            let d = (frr - far).abs();
            if d < best.0 {
                best = (d, (frr + far) / 2.0);
            }
        }
        best.1
    }

    /// One row of the S1/S2/S4 table. `v`: (S1 worst relative FRR change %, S2 max FAR delta pt,
    /// S2 max pairs >= 5 % delta, S2 any new pair >= 20 %). `harness`: the same-session tune EER
    /// ratio to the reference when the harness check flagged the model.
    fn s_row(name: &str, v: (f64, f64, i64, bool), s4: f32, harness: Option<f64>) -> String {
        let pf = |ok: bool| if ok { "pass" } else { "**FAIL**" };
        format!(
            "| {name} | {} {:+.1} % | {} {:+.3} pt, {:+}, {} | {} {s4:+.3} | {} |\n",
            pf(v.0 <= -25.0),
            v.0,
            pf(v.1 <= 0.03 && v.2 <= 1 && !v.3),
            v.1,
            v.2,
            if v.3 { "new pair" } else { "no new pair" },
            pf(s4 >= 0.0),
            harness.map_or("ok".to_string(), |r| format!("**check: EER {r:.1} x ref**")),
        )
    }

    /// Ascending copy as f64, for [`q`].
    fn sorted64<'a>(v: impl Iterator<Item = &'a f32>) -> Vec<f64> {
        let mut out: Vec<f64> = v.map(|x| f64::from(*x)).collect();
        out.sort_by(f64::total_cmp);
        out
    }

    /// Airport caller windows of one model: minimum of (score - thr) over the reproduced windows.
    fn airport_margin(
        models: &mut [Embedder],
        airport: &[AirportBase],
        m: usize,
        thr: f32,
    ) -> Result<f32, String> {
        let mut min = f32::INFINITY;
        for (ap, base) in AIRPORT.iter().zip(airport) {
            for &k in ap.caller {
                let end = base.u_ms + k * 1500;
                let e = models[m].embed(clip(&base.audio, end - 1500 - PRE_ROLL_MS, end), None)?;
                min = min.min(cosine(&base.refs[m], &e) - thr);
            }
        }
        Ok(min)
    }

    /// The four known bad callers (same-session) and the cross-session one of the caller-reject-rate
    /// plan, shown per speaker on the tune set (`test-clean`).
    const BAD_SAME: [&str; 4] = ["672", "1284", "8555", "61"];
    const BAD_CROSS: [&str; 1] = ["4446"];

    #[allow(clippy::too_many_lines, clippy::needless_range_loop)]
    pub(super) fn screen(args: &Args, root: &Path, spec: &str) -> Result<(), String> {
        if !std::env::var("FA_ENROLL").is_ok_and(|v| v == "product") {
            return Err("--models (model screen) needs FA_ENROLL=product".into());
        }
        let (dir, files) = spec
            .rsplit_once(':')
            .ok_or("--models needs <dir>:<file,...>")?;
        let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
        let reference = "nemo_en_titanet_small.onnx";
        let mut models = vec![Embedder::open(&args.speaker_dir.join(reference))?];
        for f in files
            .split(',')
            .filter(|f| !f.is_empty() && *f != reference)
        {
            models.push(Embedder::open(&Path::new(dir).join(f))?);
        }
        let tune_sp = speakers(root, "test-clean")?;
        let mut judges: Vec<(String, Vec<Speaker>)> = Vec::new();
        for j in args.judge.split(',') {
            judges.push((j.to_string(), speakers(root, j)?));
        }
        let mut airport = Vec::new();
        for (ai, ap) in AIRPORT.iter().enumerate() {
            airport.push(fit_airport(&mut models, ap, &args.airport[ai])?);
        }
        let embed_set = |models: &mut [Embedder],
                         sps: &[Speaker],
                         mode: Mode,
                         label: &str|
         -> Result<Vec<[VoiceEmb; 2]>, String> {
            let mut out = Vec::new();
            for (i, sp) in sps.iter().enumerate() {
                eprintln!("{} {label} speaker {}", mode.name(), sp.label());
                let (e, s, _, eb) = audio(sp, mode, true)?;
                out.push(embed_audio(
                    models,
                    &eb,
                    &e,
                    &s,
                    i,
                    &bed,
                    args.bed_db,
                    windows_15,
                )?);
            }
            Ok(out)
        };
        let mut md = format!(
            "### Model screen run: {}\n\nEnrollment: product (`FA_ENROLL`=product, `NM_CAND`={}). Tune: LibriSpeech `test-clean`, {} speakers; judged held-out: {}. 1.5 s windows, raw cosine. Reference: `{reference}` at {GATE_THRESHOLD}; every other model's threshold is the lowest one whose pooled tune impostor accepts are at most the reference's, per mode and condition. Bed: Tocumen airport at {} dB.\n\n",
            models[1..]
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            std::env::var("NM_CAND").unwrap_or_default(),
            tune_sp.len(),
            judges
                .iter()
                .map(|(j, s)| format!("`{j}` ({} speakers)", s.len()))
                .collect::<Vec<_>>()
                .join(", "),
            args.bed_db,
        );
        // thr[model][mode][cond]
        let mut thr = vec![[[0.0f32; 2]; 2]; models.len()];
        // Harness check: a model whose same-session tune EER is far above the reference's is
        // flagged in the S table (its scores may come from a broken extractor setup); its S
        // verdicts stand until a probe clears or condemns the harness.
        let mut flagged = vec![false; models.len()];
        let mut eer_ratio = vec![0.0f64; models.len()];
        let mut diag_md = String::from(
            "| model | mode | cond | gen p10 | gen p50 | imp p50 | imp p99.9 | EER % | sane (EER <= 3 x ref) | BAD_SAME gen p50 |\n|---|---|---|---|---|---|---|---|---|---|\n",
        );
        let mut bad_md = String::from(
            "| model | mode | speaker | FRR bed % | FRR clean % |\n|---|---|---|---|---|\n",
        );
        let mut table = String::from("| model | mode | set | cond | thr | FRR % | FAR % | FRR vs ref | FAR vs ref pt | pairs >= 5 % | pairs >= 20 % | new pair >= 20 % |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        // Verdict state over same-session set x cond: (S1 worst relative change %, S2 max FAR
        // delta pt, S2 max ge5 delta, S2 any new >= 20 pair).
        let mut verdict: Vec<(f64, f64, i64, bool)> =
            vec![(f64::NEG_INFINITY, f64::NEG_INFINITY, i64::MIN, false); models.len()];
        for (mi, mode) in [Mode::SameSession, Mode::CrossSession]
            .into_iter()
            .enumerate()
        {
            let tune = embed_set(&mut models, &tune_sp, mode, "tune")?;
            let tune_scores: Vec<Scores> =
                (0..models.len()).map(|m| raw_scores(&tune, m)).collect();
            for k in 0..2 {
                let n0 = tune_scores[0].1[k]
                    .iter()
                    .flatten()
                    .flatten()
                    .filter(|s| **s >= GATE_THRESHOLD)
                    .count();
                for m in 0..models.len() {
                    thr[m][mi][k] = if m == 0 {
                        GATE_THRESHOLD
                    } else {
                        far_matched(&tune_scores[m].1[k], n0)
                    };
                }
            }
            for k in 0..2 {
                let eers: Vec<f64> = (0..models.len())
                    .map(|m| {
                        let g: Vec<f32> = tune_scores[m].0[k].iter().flatten().copied().collect();
                        let i: Vec<f32> = tune_scores[m].1[k]
                            .iter()
                            .flatten()
                            .flatten()
                            .copied()
                            .collect();
                        eer(&g, &i)
                    })
                    .collect();
                for m in 0..models.len() {
                    let g = sorted64(tune_scores[m].0[k].iter().flatten());
                    let i = sorted64(tune_scores[m].1[k].iter().flatten().flatten());
                    let sane = eers[m] <= 3.0 * eers[0];
                    if mode == Mode::SameSession {
                        flagged[m] |= !sane;
                        eer_ratio[m] = eer_ratio[m].max(eers[m] / eers[0].max(1e-9));
                    }
                    let bad_p50 = tune_sp
                        .iter()
                        .enumerate()
                        .filter(|(_, sp)| BAD_SAME.contains(&sp.id.as_str()))
                        .map(|(a, sp)| {
                            format!(
                                "{} {:.3}",
                                sp.id,
                                q(&sorted64(tune_scores[m].0[k][a].iter()), 0.5)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    diag_md.push_str(&format!(
                        "| {} | {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {:.2} | {} | {bad_p50} |\n",
                        models[m].name,
                        mode.name(),
                        if k == 0 { "bed" } else { "clean" },
                        q(&g, 0.1),
                        q(&g, 0.5),
                        q(&i, 0.5),
                        q(&i, 0.999),
                        100.0 * eers[m],
                        if sane { "yes" } else { "**no**" },
                    ));
                }
            }
            let bad: &[&str] = if mode == Mode::SameSession {
                &BAD_SAME
            } else {
                &BAD_CROSS
            };
            for m in 0..models.len() {
                for (a, sp) in tune_sp.iter().enumerate() {
                    if bad.contains(&sp.id.as_str()) {
                        let f = |k: usize| {
                            rate_pct(rate(&tune_scores[m].0[k][a], thr[m][mi][k], false))
                        };
                        bad_md.push_str(&format!(
                            "| {} | {} | {} | {:.1} | {:.1} |\n",
                            models[m].name,
                            mode.name(),
                            sp.label(),
                            f(0),
                            f(1)
                        ));
                    }
                }
            }
            for (jname, jsp) in &judges {
                let set = embed_set(&mut models, jsp, mode, jname)?;
                for k in 0..2 {
                    let base = cell(&set, 0, thr[0][mi][k], k, jsp);
                    for m in 0..models.len() {
                        let c = cell(&set, m, thr[m][mi][k], k, jsp);
                        let (rel, dfar, dge5, new20) = (
                            100.0 * (rate_pct(c.frr) - rate_pct(base.frr))
                                / rate_pct(base.frr).max(1e-9),
                            rate_pct(c.far) - rate_pct(base.far),
                            c.ge5 as i64 - base.ge5 as i64,
                            c.ge20.iter().any(|p| !base.ge20.contains(p)),
                        );
                        if m > 0 && mode == Mode::SameSession {
                            let v = &mut verdict[m];
                            // S1 binding number: the least reduction (largest relative change).
                            v.0 = v.0.max(if base.frr.0 == 0 { 0.0 } else { rel });
                            v.1 = v.1.max(dfar);
                            v.2 = v.2.max(dge5);
                            v.3 |= new20;
                        }
                        table.push_str(&format!(
                            "| {} | {} | {jname} | {} | {:.3} | {} ({}/{}) | {} ({}/{}) | {rel:+.1} % | {dfar:+.3} | {} | {} | {} |\n",
                            models[m].name,
                            mode.name(),
                            if k == 0 { "bed" } else { "clean" },
                            thr[m][mi][k],
                            pct(c.frr.0, c.frr.1),
                            c.frr.0,
                            c.frr.1,
                            pct(c.far.0, c.far.1),
                            c.far.0,
                            c.far.1,
                            c.ge5,
                            c.ge20.len(),
                            if new20 { "**yes**" } else { "no" },
                        ));
                    }
                }
            }
        }
        md.push_str("#### Per held-out set\n\n");
        md.push_str(&table);
        md.push_str(
            "\n#### Known bad callers on the tune set (`test-clean`), at the tuned thresholds\n\n",
        );
        md.push_str(&bad_md);
        md.push_str("\n#### Tune-set score check (`test-clean`, raw cosine, harness sanity)\n\nA model with sane = no in any same-session cell is flagged in the harness column of the S table. Its S verdicts stand until a probe clears or condemns the harness (FALSE_ACCEPT_BENCH.md records which).\n\n");
        md.push_str(&diag_md);
        md.push_str("\n#### Airport A/B (reproduced offline, same-session thresholds, margin = min(score - thr))\n\nThe logged-score variant exists for TitaNet-small only (the logged cosines are its scores); for another model only the reproduced variant is defined.\n\n| model | thr bed | reproduced margin | logged margin | S4 |\n|---|---|---|---|---|\n");
        let mut s4 = vec![f32::INFINITY; models.len()];
        for m in 0..models.len() {
            let rep = airport_margin(&mut models, &airport, m, thr[m][0][0])?;
            let logged = if m == 0 {
                let l = AirScores {
                    rep: Vec::new(),
                    logged: AIRPORT
                        .iter()
                        .map(|a| {
                            a.caller
                                .iter()
                                .map(|k| a.logged[*k - 1])
                                .collect::<Vec<f32>>()
                        })
                        .collect(),
                    has_logged: true,
                };
                Some(AirScores::margin(&l.logged, thr[0][0][0]))
            } else {
                None
            };
            s4[m] = rep.min(logged.unwrap_or(f32::INFINITY));
            md.push_str(&format!(
                "| {} | {:.3} | {rep:+.3} | {} | {} |\n",
                models[m].name,
                thr[m][0][0],
                logged.map_or("n/a".to_string(), |l| format!("{l:+.3}")),
                if s4[m] >= 0.0 { "pass" } else { "**FAIL**" }
            ));
        }
        md.push_str("\n#### S1, S2, S4 (same-session, every held-out set x condition; S3 is the chunk 1 table)\n\n| model | S1 (worst relative FRR change, need <= -25 %) | S2 (max FAR delta pt, need <= +0.03; pairs >= 5 % max delta, need <= +1; new pair >= 20 %) | S4 (airport min margin) | harness (tune EER x ref) |\n|---|---|---|---|---|\n");
        for m in 1..models.len() {
            md.push_str(&s_row(
                &models[m].name,
                verdict[m],
                s4[m],
                flagged[m].then_some(eer_ratio[m]),
            ));
        }
        print!("{md}");
        if let Some(out) = &args.out {
            fs::write(out, &md).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod product_enroll_tests {
        use super::*;

        fn tone(ms: usize) -> Vec<f32> {
            (0..ms * MS).map(|i| 0.1 * (i as f32 * 0.3).sin()).collect()
        }

        #[test]
        fn trim_voiced_cuts_the_silence_around_speech() {
            let mut a = vec![0.0f32; 300 * MS];
            a.extend(tone(1000));
            a.extend(vec![0.0f32; 400 * MS]);
            let t = trim_voiced(&a).unwrap();
            assert!((t.len() / MS).abs_diff(1000) <= 20, "{}", t.len() / MS);
            assert!(trim_voiced(&vec![0.0f32; 1000 * MS]).is_none());
        }

        #[test]
        fn mode_header_names_product_enrollment() {
            assert!(mode_enroll_text(Mode::SameSession, true).contains("(product)"));
            assert!(mode_enroll_text(Mode::CrossSession, true).contains("(product)"));
            assert_eq!(
                mode_enroll_text(Mode::SameSession, false),
                "Enrollment: the first 3000 ms from the voiced onset of the speaker's longest chapter; windows: the rest of that chapter after the enrollment span (no overlap). "
            );
        }

        #[test]
        fn product_enrollment_needs_c2_or_span_flag_only_with_product_mode() {
            // The default NM_CAND is today's rules: the span is the gate's 3000 ms.
            let c = cand::parse_cand("", 0.35).unwrap();
            assert_eq!(c.enroll_ms, ENROLL_MS);
            assert!(!c.c2);
            assert!(cand::parse_cand("c2,c4n3", 0.35).unwrap().c2);
        }
    }

    #[cfg(test)]
    mod screen_tests {
        use super::*;

        #[test]
        fn eer_of_separable_scores_is_zero() {
            assert_eq!(eer(&[0.9, 0.8], &[0.1, 0.2]), 0.0);
        }

        #[test]
        fn eer_of_identical_scores_is_half() {
            assert_eq!(eer(&[0.5, 0.5], &[0.5, 0.5]), 0.5);
        }

        #[test]
        fn unverified_passing_row_is_flagged() {
            let r = s_row("m", (-30.0, 0.0, 0, false), 0.1, Some(5.0));
            assert!(r.contains("pass"), "{r}");
            assert!(r.contains("check: EER 5.0"), "{r}");
        }

        #[test]
        fn verified_failing_row_says_fail() {
            let r = s_row("m", (10.0, 0.0, 0, false), 0.1, None);
            assert!(r.contains("**FAIL**"), "{r}");
            assert!(r.trim_end().ends_with("| ok |"), "{r}");
        }

        #[test]
        fn gate_threshold_is_the_product_default() {
            assert_eq!(
                GATE_THRESHOLD,
                node_webrtc_rust_speech::config::SpeakerGateConfig::default().threshold
            );
        }

        #[test]
        fn map_imp_is_identity_in_survival_for_equal_sets() {
            let r = vec![vec![vec![0.9f32, 0.6], vec![0.8, 0.7]]];
            let (n, m) = map_imp(&r, &r, 0.75).expect("survivors");
            assert_eq!(n, 2);
            let flat: Vec<f32> = r.iter().flatten().flatten().copied().collect();
            assert_eq!(rate(&flat, m, true).0, 2);
        }

        #[test]
        fn map_imp_follows_a_monotone_rescale() {
            let r = vec![vec![vec![0.9f32, 0.6], vec![0.8, 0.7]]];
            let t: Vec<Vec<Vec<f32>>> = r
                .iter()
                .map(|a| {
                    a.iter()
                        .map(|b| b.iter().map(|x| 0.5 * x + 0.1).collect())
                        .collect()
                })
                .collect();
            let tf: Vec<f32> = t.iter().flatten().flatten().copied().collect();
            let rf: Vec<f32> = r.iter().flatten().flatten().copied().collect();
            let (n1, m1) = map_imp(&r, &t, 0.65).unwrap();
            let (n2, m2) = map_imp(&r, &t, 0.85).unwrap();
            assert_eq!((n1, n2), (3, 1));
            assert_eq!(rate(&tf, m1, true).0, n1);
            assert_eq!(rate(&tf, m2, true).0, n2);
            assert_eq!(rate(&rf, 0.65, true).0, n1);
            assert!(m1 <= m2, "{m1} {m2}");
        }

        #[test]
        fn map_imp_zero_survivors_is_none() {
            let r = vec![vec![vec![0.9f32, 0.6], vec![0.8, 0.7]]];
            assert_eq!(map_imp(&r, &r, 0.95), None);
        }

        #[test]
        fn map_gen_matches_genuine_survival() {
            let r = [0.5f32, 0.6, 0.7, 0.9];
            let t: Vec<f32> = r.iter().map(|x| x + 0.05).collect();
            let (n, m) = map_gen(&r, &t, 0.65);
            assert_eq!(n, 2);
            assert_eq!(t.iter().filter(|x| **x >= m).count(), 2);
        }

        #[test]
        fn far_matched_admits_exactly_n0() {
            let imp = vec![vec![vec![0.9f32, 0.6], vec![0.8, 0.7]]];
            let flat: Vec<f32> = imp.iter().flatten().flatten().copied().collect();
            let thr = far_matched(&imp, 2);
            assert!(thr > 0.7 && thr <= 0.8, "{thr}");
            assert_eq!(rate(&flat, thr, true).0, 2);
            assert_eq!(rate(&flat, far_matched(&imp, 0), true).0, 0);
            assert_eq!(far_matched(&imp, 4), f32::MIN);
        }
    }
}
