//! Caller -> other noise-window bench (plan `2026-11-13-speaker-split-other-to-caller`, decisions
//! n and o, chunk 6c): which per-window signal tells a real bystander takeover after the caller
//! from bed-only windows after a short caller phrase, so a `caller` watch can book the bed windows
//! as "no evidence" (they neither confirm nor reset a caller -> other change)?
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
//!   --models <speaker dir> --main-tts <amy-low dir> --phrases <stt-phrase-qualification.json> \
//!   --out <dir> --caller-to-other 125 --s4-bed <tocumen48.f32> --s4-bed-db -12
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example caller_to_other_bench -- \
//!   --fixtures <dir> --model <nemo_en_titanet_small.onnx> [--threshold 0.459] [--out <file.md>]
//! ```
//!
//! Per S5 scene (`caller_to_other` takeover or `caller_tail`, see `speaker_fixtures`):
//! 1. The 48 kHz scene runs through the speech crate's `VadEngine` with the airport demo's VAD
//!    (adaptive, threshold 0.05, 200 ms min speech, 1300 ms min silence). It records every 20 ms
//!    frame's `active` flag (the second value of `process_webrtc_pcm`) and the utterance that
//!    holds the caller's voiced onset. A takeover whose utterance ends before the bystander's onset
//!    is `separate` (the gate scores the bystander as a new key).
//! 2. The gate's window grid: window `k` (1-based) covers `[U + (k-1) * 1500, U + k * 1500)` ms,
//!    `U` = SpeechStart minus the minimum speech duration; the clip adds 300 ms of pre-roll. Each
//!    clip is embedded (TitaNet) and scored by cosine against the enrollment sentence; `caller` =
//!    score >= threshold. Window 1 labels the key; a key labelled `other` is `first other`.
//! 3. Per window: the energy share at D dB (share of the clip's 20 ms frames at or above the key's
//!    caller level minus D; the caller level is the 90th-percentile frame dB of the loudest
//!    `caller` window so far, as in the speaker gate scaffolding), the VAD-active share (share of
//!    the clip's frames with `active` set), and the score.
//! 4. The caller -> other watch per rule (signal x threshold): an `other` window under the
//!    threshold (score rule, exploratory: an `other` window scoring above it) is no evidence: it
//!    neither confirms nor resets the run and does not count against the 12 windows. A `caller`
//!    window resets the run. Two consecutive evidence windows `other` confirm the change at the end
//!    of the second one. Threshold 0 (score: the gate threshold) is today's gate.
//!
//! Takeovers: `kept` (today's gate and the rule both confirm), `lost` (today confirms, the rule
//! does not), `delayed` (kept, the rule confirms later; delay in ms), `early` (the rule confirms on
//! windows that all end before the bystander's onset, a bed split). Tails: `blocked` (no change
//! confirmed) or `let through`. Pairs with FAR >= 5 % in `FALSE_ACCEPT_BENCH.md` (amy-low <-
//! libritts_r 120, 51.5 % bed) are left out of the main table and reported on their own.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

#[path = "support/mod.rs"]
mod support;

use support::{parse_json, read_wav, read_wav_stereo_48k_pcm, Json};

const SR: i32 = 16_000;
const SAMPLES_PER_MS: usize = 16;
const WINDOW_MS: usize = 1500;
const PRE_ROLL_MS: usize = 300;
const FRAME_MS: usize = 20;
/// `recheck_max_windows` default: counted windows per utterance, the first included.
const MAX_COUNTED: usize = 12;
/// Scored windows per utterance at most (no-evidence windows are not counted against 12).
const MAX_SCORED: usize = 30;
/// `SPEAKER_CHANGE_CONFIRM_WINDOWS`.
const CONFIRM: usize = 2;
/// Energy share D values (dB below the caller's p90 level).
const DS: [f64; 3] = [12.0, 18.0, 24.0];
/// Verify budget per window (`verify_timeout_ms` default).
const VERIFY_MS: f64 = 500.0;

struct Embedder {
    extractor: SpeakerEmbeddingExtractor,
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
        Ok(Self { extractor })
    }

    /// Embedding of `samples` and the seconds it took.
    fn embed(&self, samples: &[f32]) -> Option<(Vec<f32>, f64)> {
        let start = Instant::now();
        let stream = self.extractor.create_stream()?;
        stream.accept_waveform(SR, samples);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return None;
        }
        let embedding = self.extractor.compute(&stream)?;
        Some((embedding, start.elapsed().as_secs_f64()))
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

/// `[start, end)` ms of `samples`, clamped.
fn slice(samples: &[f32], start_ms: usize, end_ms: usize) -> &[f32] {
    let a = (start_ms * SAMPLES_PER_MS).min(samples.len());
    let b = (end_ms * SAMPLES_PER_MS).min(samples.len()).max(a);
    &samples[a..b]
}

/// 20 ms frame powers (dB) of a clip, as the gate's `frame_db` (exact frames only).
fn frame_db(clip: &[f32]) -> Vec<f64> {
    clip.as_chunks::<{ FRAME_MS * SAMPLES_PER_MS }>()
        .0
        .iter()
        .map(|f| {
            let p = f.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / f.len() as f64;
            10.0 * p.max(1e-12).log10()
        })
        .collect()
}

fn p90(d: &[f64]) -> Option<f64> {
    if d.is_empty() {
        return None;
    }
    let mut d = d.to_vec();
    d.sort_by(f64::total_cmp);
    Some(d[((d.len() - 1) as f64 * 0.9).round() as usize])
}

fn share_at(d: &[f64], level: f64, below: f64) -> f64 {
    if d.is_empty() {
        return 0.0;
    }
    d.iter().filter(|v| **v >= level - below).count() as f64 / d.len() as f64
}

fn num(j: &Json, key: &str) -> Option<f64> {
    match j.get(key)? {
        Json::Num(n) => Some(*n),
        _ => None,
    }
}

fn text(j: &Json, key: &str) -> Option<String> {
    match j.get(key)? {
        Json::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn demo_vad_config() -> VadConfig {
    VadConfig {
        provider: "adaptive".into(),
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: 1300,
        ..VadConfig::default()
    }
}

/// Per 20 ms frame `active` flags and the utterances `[start, end)` ms.
/// Per-frame `active` flags and utterances.
type VadRun = (Vec<bool>, Vec<(usize, usize)>);

fn run_vad(pcm48: &[u8]) -> Result<VadRun, String> {
    let config = demo_vad_config();
    let mut vad = VadEngine::new(config.clone()).map_err(|e| e.to_string())?;
    let frame_bytes = 960 * 2 * 2;
    let mut start = None;
    let mut utts = Vec::new();
    let mut active = Vec::new();
    for (i, frame) in pcm48.chunks_exact(frame_bytes).enumerate() {
        let ms = i * FRAME_MS;
        let (transitions, a) = vad
            .process_webrtc_pcm(frame, 20)
            .map_err(|e| e.to_string())?;
        active.push(a);
        for tr in transitions {
            match tr {
                VadTransition::SpeechStart => {
                    start = Some((ms + 20).saturating_sub(config.min_speech_duration_ms as usize))
                }
                VadTransition::SpeechEnd => {
                    if let Some(s) = start.take() {
                        utts.push((s, ms + 20));
                    }
                }
            }
        }
    }
    if let Some(s) = start {
        utts.push((s, active.len() * FRAME_MS));
    }
    Ok((active, utts))
}

/// One scored window.
struct Win {
    start: usize,
    score: f32,
    db: Vec<f64>,
    vad_share: f64,
}

impl Win {
    fn end(&self) -> usize {
        self.start + WINDOW_MS
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Sig {
    Energy(usize),
    Vad,
    Score,
}

fn sig_name(s: Sig) -> String {
    match s {
        Sig::Energy(i) => format!("energy D={}", DS[i]),
        Sig::Vad => "VAD-active share".into(),
        Sig::Score => "score (exploratory)".into(),
    }
}

fn sig_thresholds(s: Sig) -> Vec<f64> {
    match s {
        Sig::Score => vec![
            0.459, 0.40, 0.35, 0.30, 0.25, 0.20, 0.15, 0.10, 0.05, 0.0, -0.05, -0.10,
        ],
        _ => (0..=19).map(|i| f64::from(i) * 0.05).collect(),
    }
}

/// Signal value of `w` for the key's caller level.
fn sig_value(s: Sig, w: &Win, level: f64) -> f64 {
    match s {
        Sig::Energy(i) => share_at(&w.db, level, DS[i]),
        Sig::Vad => w.vad_share,
        Sig::Score => f64::from(w.score),
    }
}

/// `other` window is no evidence under the rule.
fn no_evidence(s: Sig, t: f64, w: &Win, level: f64) -> bool {
    match s {
        Sig::Score => f64::from(w.score) > t,
        _ => sig_value(s, w, level) < t,
    }
}

/// Confirmed change: (first window index of the run, confirming window index).
type Confirm = Option<(usize, usize)>;

/// Caller watch over `wins` (window 1 already `caller`).
fn simulate(wins: &[Win], thr: f32, rule: Option<(Sig, f64)>) -> Confirm {
    let mut level = p90(&wins[0].db).unwrap_or(-120.0);
    let mut counted = 1;
    let mut run = 0;
    let mut first = 0;
    for (i, w) in wins.iter().enumerate().skip(1) {
        if counted >= MAX_COUNTED {
            break;
        }
        if w.score >= thr {
            run = 0;
            if let Some(l) = p90(&w.db) {
                level = level.max(l);
            }
            counted += 1;
            continue;
        }
        if let Some((s, t)) = rule {
            if no_evidence(s, t, w, level) {
                continue;
            }
        }
        counted += 1;
        if run == 0 {
            first = i;
        }
        run += 1;
        if run >= CONFIRM {
            return Some((first, i));
        }
    }
    None
}

/// Caller level at each window (as `simulate` tracks it, for the signal distributions).
fn levels(wins: &[Win], thr: f32) -> Vec<f64> {
    let mut level = p90(&wins[0].db).unwrap_or(-120.0);
    let mut out = vec![level];
    for w in wins.iter().skip(1) {
        out.push(level);
        if w.score >= thr {
            if let Some(l) = p90(&w.db) {
                level = level.max(l);
            }
        }
    }
    out
}

/// Gate `SPLIT_SMOOTH_FRAMES`.
const SMOOTH: usize = 5;

/// Where in the longest quiet run the cut lands.
#[derive(Clone, Copy)]
enum Pos {
    End,
    Start,
    /// `end - ms`, not before the run's start.
    Back(usize),
    Mid,
    /// The lowest-energy 20 ms frame of the run (ties: the later one).
    MinEnergy,
    /// The run's end, or, when the run reaches the range end (no loud frame after it: the next
    /// speaker or the bed, not a pause), its start + `ms` (chunk 6e, parent decision).
    StartIfOpen(usize),
    /// Raw-frame cut (chunk 6e, parent decision): a closed run cuts right before the next
    /// speaker's first loud raw frame (the smoothing offset undone); an open run (quiet to the
    /// range end) cuts at the end of the caller's last loud raw frame + `ms`.
    Raw(usize),
    /// Hybrid (chunk 6e, decision r): a closed run cuts as [`Pos::Raw`]; an open run cuts at the
    /// run's end (the range end), as [`Pos::End`].
    Hybrid,
    /// [`Pos::Hybrid`] with the open-run cut capped at the first `other` window's start + `ms`
    /// (chunk 6e, decision s).
    HybridCap(usize),
    /// [`Pos::HybridCap`], but the capped cut never lands before the open run's raw start (its
    /// first frame after any loud raw frames within the smoothing reach): when the
    /// caller is still loud past the cap, the cut waits for the quiet run (parent decision after
    /// decision s).
    HybridCapClamp(usize),
}

fn pos_name(p: Pos) -> String {
    match p {
        Pos::End => "end".into(),
        Pos::Start => "start".into(),
        Pos::Back(ms) => format!("end - {ms} ms"),
        Pos::Mid => "midpoint".into(),
        Pos::MinEnergy => "lowest-energy frame".into(),
        Pos::StartIfOpen(ms) => format!("end; start + {ms} ms when it reaches the range end"),
        Pos::Raw(ms) => {
            format!("raw: closed = first loud raw frame; open = last loud raw frame + {ms} ms")
        }
        Pos::Hybrid => "hybrid: closed = first loud raw frame; open = run end".into(),
        Pos::HybridCap(ms) => format!("hybrid, open capped at first other start + {ms} ms"),
        Pos::HybridCapClamp(ms) => {
            format!("hybrid, open capped at first other start + {ms} ms, not before the run start")
        }
    }
}

/// Caller -> other cut candidates (chunk 6e, decision q).
#[derive(Clone, Copy)]
enum Cut {
    /// Today's gate: lowest-energy frame within +-300 ms of the first `other` window's start.
    Today,
    /// Longest quiet run (the gate's stage 1 rule) in `[first start - x, end)`, `end` the first
    /// `other` window's end (`confirm`: the confirming window's end); cut at the run's end
    /// (see [`Pos`]).
    /// `mid_hi`: the range ends at the first `other` window's midpoint instead. `early` > 0: the
    /// earliest run at least `early` x the longest one.
    Quiet {
        x: usize,
        confirm: bool,
        pos: Pos,
        mid_hi: bool,
        early: f64,
    },
    /// First quiet run of at least `min` ms in `[first start - x, first end)`; cut at its start.
    FirstQuiet { x: usize, min: usize },
}

const CUTS: [Cut; 53] = [
    Cut::Today,
    Cut::Quiet {
        x: 300,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 300,
        confirm: false,
        pos: Pos::Back(60),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 300,
        confirm: false,
        pos: Pos::Back(100),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 300,
        confirm: false,
        pos: Pos::Back(160),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 300,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 300,
        confirm: false,
        pos: Pos::Start,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Back(60),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Back(100),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Back(160),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Start,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Back(60),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Back(100),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Back(160),
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Start,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1800,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::MinEnergy,
        mid_hi: false,
        early: 0.0,
    },
    Cut::FirstQuiet { x: 750, min: 300 },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::End,
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.5,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.5,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::End,
        mid_hi: true,
        early: 0.5,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: true,
        early: 0.5,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.7,
    },
    Cut::Quiet {
        x: 750,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.7,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::End,
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.5,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.5,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::End,
        mid_hi: true,
        early: 0.5,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: true,
        early: 0.5,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::End,
        mid_hi: false,
        early: 0.7,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Mid,
        mid_hi: false,
        early: 0.7,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::StartIfOpen(0),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::StartIfOpen(60),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::StartIfOpen(120),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::StartIfOpen(200),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Raw(0),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Raw(40),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Raw(80),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Raw(120),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::Hybrid,
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::HybridCap(0),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::HybridCap(150),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::HybridCap(300),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::HybridCap(450),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::HybridCap(600),
        mid_hi: true,
        early: 0.0,
    },
    Cut::Quiet {
        x: 1500,
        confirm: false,
        pos: Pos::HybridCapClamp(450),
        mid_hi: true,
        early: 0.0,
    },
];

fn cut_name(c: Cut) -> String {
    match c {
        Cut::Today => "today: min-energy frame +-300 ms of the first other start".into(),
        Cut::Quiet {
            x,
            confirm,
            pos,
            mid_hi,
            early,
        } => format!(
            "{} quiet run [first start - {x}, {}), cut at run {}",
            if early > 0.0 {
                format!("earliest >= {early} x longest")
            } else {
                "longest".into()
            },
            if mid_hi {
                "first other mid"
            } else if confirm {
                "confirm end"
            } else {
                "first other end"
            },
            pos_name(pos)
        ),
        Cut::FirstQuiet { x, min } => {
            format!("first quiet run >= {min} ms [first start - {x}, first other end), cut at run start")
        }
    }
}

/// Mean square per 20 ms frame of `[lo, lo + n * 20)` ms.
fn frame_powers(samples: &[f32], lo: usize, hi: usize) -> Vec<f64> {
    let n = hi.saturating_sub(lo) / FRAME_MS;
    (0..n)
        .map(|i| {
            let f = slice(samples, lo + i * FRAME_MS, lo + (i + 1) * FRAME_MS);
            if f.is_empty() {
                0.0
            } else {
                f.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / f.len() as f64
            }
        })
        .collect()
}

/// Quiet flags per frame of `[lo, hi)`, the gate's stage 1 rule: power smoothed over 5 frames
/// (dB), quiet below the midpoint of the range's 10th and 90th percentile.
fn quiet_flags(samples: &[f32], lo: usize, hi: usize) -> Vec<bool> {
    let raw = frame_powers(samples, lo, hi);
    let n = raw.len();
    if n == 0 {
        return Vec::new();
    }
    let db = |p: f64| 10.0 * p.max(1e-12).log10();
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(SMOOTH / 2);
            let b = (i + SMOOTH / 2 + 1).min(n);
            db(raw[a..b].iter().sum::<f64>() / (b - a) as f64)
        })
        .collect();
    let mut sorted = smooth.clone();
    sorted.sort_by(f64::total_cmp);
    let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let mid = (pct(0.1) + pct(0.9)) / 2.0;
    smooth.iter().map(|v| *v < mid).collect()
}

/// Raw-frame cut over `[lo, hi)` (see [`Pos::Raw`]): (cut, open run). `None` without a run.
fn raw_cut(samples: &[f32], lo: usize, hi: usize, m: usize) -> Option<(usize, bool)> {
    let raw = frame_powers(samples, lo, hi);
    let n = raw.len();
    if n == 0 {
        return None;
    }
    let db = |p: f64| 10.0 * p.max(1e-12).log10();
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(SMOOTH / 2);
            let b = (i + SMOOTH / 2 + 1).min(n);
            db(raw[a..b].iter().sum::<f64>() / (b - a) as f64)
        })
        .collect();
    let mut sorted = smooth.clone();
    sorted.sort_by(f64::total_cmp);
    let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let mid = (pct(0.1) + pct(0.9)) / 2.0;
    let (mut best, mut best_end, mut run) = (0, 0, 0);
    for (i, v) in smooth.iter().enumerate() {
        if *v < mid {
            run += 1;
            if run >= best {
                best = run;
                best_end = i + 1;
            }
        } else {
            run = 0;
        }
    }
    if best == 0 {
        return None;
    }
    let loud = |i: usize| db(raw[i]) >= mid;
    let start = best_end - best;
    if best_end < n {
        // Closed: the first loud raw frame at or after the smoothed end, within the smoothing reach
        // either side.
        let from = best_end.saturating_sub(SMOOTH / 2).max(start);
        let to = (best_end + SMOOTH / 2).min(n);
        let at = (from..to).find(|i| loud(*i)).unwrap_or(best_end);
        return Some((lo + at * FRAME_MS, false));
    }
    // Open: the end of the last loud raw frame before the run (within the smoothing reach).
    let reach = (start + SMOOTH / 2).min(n);
    let last = (0..reach).rev().find(|i| loud(*i));
    let at = last.map_or(lo + start * FRAME_MS, |i| lo + (i + 1) * FRAME_MS);
    Some(((at + m).min(lo + n * FRAME_MS), true))
}

/// Raw start (ms) of the longest quiet run in `[lo, hi)`: its first frame after any loud raw frames
/// within the smoothing reach of the smoothed start (as the gate's `QuietScan::raw_cut`).
fn raw_open_start(samples: &[f32], lo: usize, hi: usize) -> Option<usize> {
    let raw = frame_powers(samples, lo, hi);
    let n = raw.len();
    let db = |p: f64| 10.0 * p.max(1e-12).log10();
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(SMOOTH / 2);
            let b = (i + SMOOTH / 2 + 1).min(n);
            db(raw[a..b].iter().sum::<f64>() / (b - a) as f64)
        })
        .collect();
    let mut sorted = smooth.clone();
    sorted.sort_by(f64::total_cmp);
    let pct = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    let mid = (pct(0.1) + pct(0.9)) / 2.0;
    let (mut best, mut best_end, mut run) = (0, 0, 0);
    for (i, v) in smooth.iter().enumerate() {
        if *v < mid {
            run += 1;
            if run >= best {
                best = run;
                best_end = i + 1;
            }
        } else {
            run = 0;
        }
    }
    if best == 0 {
        return None;
    }
    let start = best_end - best;
    let to = (start + SMOOTH / 2).min(n);
    let at = (start..to)
        .rev()
        .find(|i| db(raw[*i]) >= mid)
        .map_or(start, |i| i + 1);
    Some(lo + at * FRAME_MS)
}

/// (start, end) ms of the longest quiet run in `[lo, hi)` (ties: the later run).
fn longest_quiet(samples: &[f32], lo: usize, hi: usize) -> Option<(usize, usize)> {
    let q = quiet_flags(samples, lo, hi);
    let (mut best, mut best_end, mut run) = (0, None, 0);
    for (i, v) in q.iter().enumerate() {
        if *v {
            run += 1;
            if run >= best {
                best = run;
                best_end = Some(i + 1);
            }
        } else {
            run = 0;
        }
    }
    best_end
        .filter(|_| best > 0)
        .map(|e| (lo + (e - best) * FRAME_MS, lo + e * FRAME_MS))
}

/// Quiet runs `(start, end)` ms of `[lo, hi)`.
fn quiet_runs(samples: &[f32], lo: usize, hi: usize) -> Vec<(usize, usize)> {
    let q = quiet_flags(samples, lo, hi);
    let mut out = Vec::new();
    let mut start = None;
    for (i, v) in q.iter().chain(std::iter::once(&false)).enumerate() {
        match (*v, start) {
            (true, None) => start = Some(i),
            (false, Some(a)) => {
                out.push((lo + a * FRAME_MS, lo + i * FRAME_MS));
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// `early` 0: the longest quiet run (ties later). Else the earliest run at least `early` x the
/// longest.
fn pick_quiet(samples: &[f32], lo: usize, hi: usize, early: f64) -> Option<(usize, usize)> {
    if early <= 0.0 {
        return longest_quiet(samples, lo, hi);
    }
    let runs = quiet_runs(samples, lo, hi);
    let max = runs.iter().map(|(a, b)| b - a).max()?;
    runs.into_iter()
        .find(|(a, b)| (b - a) as f64 >= early * max as f64)
}

/// Start of the first quiet run of at least `min` ms in `[lo, hi)`.
fn first_quiet(samples: &[f32], lo: usize, hi: usize, min: usize) -> Option<usize> {
    let q = quiet_flags(samples, lo, hi);
    let need = min.div_ceil(FRAME_MS);
    let mut run = 0;
    for (i, v) in q.iter().enumerate() {
        run = if *v { run + 1 } else { 0 };
        if run >= need {
            return Some(lo + (i + 1 - run) * FRAME_MS);
        }
    }
    None
}

/// Today's `split_point`: lowest-energy 20 ms frame of the clip `[start - 300, start + 1500)`
/// within +-300 ms of `start` (ties: nearest `start`, then earlier).
fn today_cut(samples: &[f32], start: usize) -> usize {
    let a = start.saturating_sub(PRE_ROLL_MS);
    let p = frame_powers(samples, a, start + WINDOW_MS);
    let mut best: Option<(f64, usize, usize)> = None;
    for (i, e) in p.iter().enumerate() {
        let t = a + i * FRAME_MS;
        if t + 300 < start || t > start + 300 {
            continue;
        }
        let cand = (*e, t.abs_diff(start), t);
        if best.is_none_or(|b| cand.partial_cmp(&b) == Some(std::cmp::Ordering::Less)) {
            best = Some(cand);
        }
    }
    best.map_or(start, |b| b.2)
}

/// Cut of candidate `c` for a change confirmed on windows `first` .. `confirm`; falls back to
/// today's cut without a quiet run.
/// (cut, whether a `StartIfOpen` cut took the run-start branch).
fn cut_at(c: Cut, samples: &[f32], wins: &[Win], first: usize, confirm: usize) -> (usize, bool) {
    let s = wins[first].start;
    let today = || today_cut(samples, s);
    let mut open = false;
    let at = match c {
        Cut::Today => today(),
        Cut::Quiet {
            x,
            confirm: c2,
            pos,
            mid_hi,
            early,
        } => {
            let hi = if mid_hi {
                s + WINDOW_MS / 2
            } else if c2 {
                wins[confirm].end()
            } else {
                wins[first].end()
            };
            let lo = s.saturating_sub(x);
            if let Pos::Raw(m) = pos {
                let (at, o) = raw_cut(samples, lo, hi, m).unwrap_or_else(|| (today(), false));
                return (at, o);
            }
            if let Pos::Hybrid | Pos::HybridCap(_) | Pos::HybridCapClamp(_) = pos {
                let (cap, clamp) = match pos {
                    Pos::HybridCap(c) => (s + c, false),
                    Pos::HybridCapClamp(c) => (s + c, true),
                    _ => (usize::MAX, false),
                };
                let raw_start = raw_open_start(samples, lo, hi).unwrap_or(0);
                return match raw_cut(samples, lo, hi, 0) {
                    Some((at, false)) => (at, false),
                    Some((_, true)) => (
                        pick_quiet(samples, lo, hi, 0.0).map_or_else(today, |(a, b)| {
                            let capped = b.min(cap);
                            let a = a.max(raw_start);
                            if clamp && capped < a {
                                eprintln!("clamp: cut moved from {capped} to the run start {a}");
                                a
                            } else {
                                capped
                            }
                        }),
                        true,
                    ),
                    None => (today(), false),
                };
            }
            let range_end = lo + (hi.saturating_sub(lo) / FRAME_MS) * FRAME_MS;
            pick_quiet(samples, lo, hi, early).map_or_else(today, |(a, b)| match pos {
                Pos::StartIfOpen(ms) if b >= range_end => {
                    open = true;
                    (a + ms).min(b)
                }
                Pos::StartIfOpen(_) => b,
                Pos::Raw(_) | Pos::Hybrid | Pos::HybridCap(_) | Pos::HybridCapClamp(_) => {
                    unreachable!("handled above")
                }
                Pos::End => b,
                Pos::Start => a,
                Pos::Back(ms) => b.saturating_sub(ms).max(a),
                Pos::Mid => (a + b) / 2,
                Pos::MinEnergy => {
                    let p = frame_powers(samples, a, b);
                    let mut best = (f64::INFINITY, b);
                    for (i, e) in p.iter().enumerate() {
                        if *e <= best.0 {
                            best = (*e, a + i * FRAME_MS);
                        }
                    }
                    best.1
                }
            })
        }
        Cut::FirstQuiet { x, min } => {
            first_quiet(samples, s.saturating_sub(x), wins[first].end(), min).unwrap_or_else(today)
        }
    };
    (at, open)
}

/// Words of `(on, off, words)` whose midpoint is before `t` (uniform word timing, as in
/// `split_point_bench`).
fn words_before(span: (usize, usize, usize), t: usize) -> usize {
    let (on, off, words) = span;
    let d = off.saturating_sub(on) as f64;
    (0..words)
        .filter(|j| (on as f64 + (*j as f64 + 0.5) * d / words as f64) < t as f64)
        .count()
}

/// Per candidate: the cut on today's confirmed change, and its cost in seconds.
struct Cuts {
    at: Vec<usize>,
    open: Vec<bool>,
    secs: Vec<f64>,
}

struct Scene {
    tail: bool,
    by_db: i64,
    pair: String,
    excluded: bool,
    by_on: usize,
    wins: Vec<Win>,
    /// Caller phrase (voiced on, off, words) and bystander (same; `None` on tails).
    caller: (usize, usize, usize),
    by: Option<(usize, usize, usize)>,
    /// Cuts of today's confirmed change (`None`: today's gate confirms none).
    cuts: Option<Cuts>,
}

enum Status {
    Ok(Box<Scene>),
    NoVad,
    Separate,
    FirstOther,
    Short,
}

struct Costs {
    embed_s: Vec<f64>,
    energy_s: Vec<f64>,
}

fn analyze(
    fixtures: &Path,
    scene: &Json,
    embedder: &Embedder,
    thr: f32,
    costs: &mut Costs,
) -> Result<(String, bool, Status), String> {
    let id = text(scene, "id").ok_or("scene id")?;
    let tail = text(scene, "kind").as_deref() == Some("caller_tail");
    let segs = match scene.get("segments") {
        Some(Json::Arr(a)) => a,
        _ => return Err(format!("{id}: no segments")),
    };
    let role = |s: &Json| text(s, "role").unwrap_or_default();
    let enrolled = |s: &Json| matches!(s.get("enroll"), Some(Json::Bool(true)));
    let span = |s: &Json| -> Option<(usize, usize)> {
        Some((
            num(s, "voiced_start_ms")? as usize,
            num(s, "voiced_end_ms")? as usize,
        ))
    };
    let enroll = segs
        .iter()
        .find(|s| enrolled(s))
        .and_then(span)
        .ok_or(format!("{id}: no enrollment"))?;
    let caller = segs
        .iter()
        .find(|s| role(s) == "main" && !enrolled(s))
        .and_then(span)
        .ok_or(format!("{id}: no caller"))?;
    let words = |s: &Json| text(s, "text").map_or(1, |t| t.split_whitespace().count().max(1));
    let caller_words = segs
        .iter()
        .find(|s| role(s) == "main" && !enrolled(s))
        .map_or(1, words);
    let by_seg = segs.iter().find(|s| role(s) == "bystander");
    let by = by_seg.and_then(|b| span(b).map(|(a, e)| (a, e, words(b))));
    let (pair, excluded, by_on) = match by_seg {
        Some(b) => {
            let voice = text(b, "voice").unwrap_or_default();
            let sid = num(b, "sid").unwrap_or(0.0) as i64;
            let short = voice
                .trim_start_matches("en_US-")
                .trim_end_matches("-medium")
                .to_string();
            let name = if short == "libritts_r" {
                format!("amy-low ← libritts_r {sid}")
            } else {
                format!("amy-low ← {short}")
            };
            (
                name,
                short == "libritts_r" && sid == 120,
                span(b).ok_or("bystander span")?.0,
            )
        }
        None => ("-".into(), false, usize::MAX),
    };
    let samples = read_wav(&fixtures.join("scenes").join(format!("{id}.16k.wav")))?;
    let pcm48 = read_wav_stereo_48k_pcm(&fixtures.join("scenes").join(format!("{id}.wav")))?;
    let (active, utts) = run_vad(&pcm48)?;
    let Some(&(u, u_end)) = utts.iter().find(|(_, e)| caller.0 < *e) else {
        return Ok((id, tail, Status::NoVad));
    };
    if u > caller.1 {
        return Ok((id, tail, Status::NoVad));
    }
    if !tail && u_end <= by_on {
        return Ok((id, tail, Status::Separate));
    }
    let (reference, _) = embedder
        .embed(slice(&samples, enroll.0, enroll.1))
        .ok_or("enroll embed")?;
    let mut wins = Vec::new();
    let mut k = 1;
    while k <= MAX_SCORED && u + k * WINDOW_MS <= u_end {
        let start = u + (k - 1) * WINDOW_MS;
        let a = start.saturating_sub(PRE_ROLL_MS);
        let b = start + WINDOW_MS;
        let clip = slice(&samples, a, b);
        let (score, secs) = embedder
            .embed(clip)
            .map_or((0.0, 0.0), |(e, s)| (cosine(&e, &reference), s));
        costs.embed_s.push(secs);
        let t0 = Instant::now();
        let db = frame_db(clip);
        let lvl = p90(&db).unwrap_or(0.0);
        let _ = std::hint::black_box(share_at(&db, lvl, 18.0));
        costs.energy_s.push(t0.elapsed().as_secs_f64());
        let frames = &active[(a / FRAME_MS).min(active.len())..(b / FRAME_MS).min(active.len())];
        let vad_share = if frames.is_empty() {
            0.0
        } else {
            frames.iter().filter(|x| **x).count() as f64 / frames.len() as f64
        };
        wins.push(Win {
            start,
            score,
            db,
            vad_share,
        });
        k += 1;
    }
    eprintln!(
        "{id}: utt {u}-{u_end} caller {}-{} by_on {} windows {:?}",
        caller.0,
        caller.1,
        if by_on == usize::MAX { 0 } else { by_on },
        wins.iter()
            .map(|w| format!("{}:{:.3}/v{:.2}", w.start, w.score, w.vad_share))
            .collect::<Vec<_>>()
    );
    if wins.is_empty() {
        return Ok((id, tail, Status::Short));
    }
    if wins[0].score < thr {
        return Ok((id, tail, Status::FirstOther));
    }
    let cuts = simulate(&wins, thr, None).map(|(f, c)| {
        let mut at = Vec::new();
        let mut open = Vec::new();
        let mut secs = Vec::new();
        for cand in CUTS {
            let t0 = Instant::now();
            let (cut, o) = cut_at(cand, &samples, &wins, f, c);
            at.push(cut);
            open.push(o);
            secs.push(t0.elapsed().as_secs_f64());
        }
        Cuts { at, open, secs }
    });
    Ok((
        id,
        tail,
        Status::Ok(Box::new(Scene {
            tail,
            by_db: num(scene, "level_db").unwrap_or(0.0) as i64,
            pair,
            excluded,
            by_on,
            wins,
            caller: (caller.0, caller.1, caller_words),
            by,
            cuts,
        })),
    ))
}

#[derive(Default, Clone)]
struct Tally {
    base_kept: usize,
    base_missed: usize,
    kept: usize,
    lost: usize,
    delayed: usize,
    delays: Vec<usize>,
    early: usize,
    tails: usize,
    blocked: usize,
    let_through: usize,
}

fn tally<'a>(scenes: impl Iterator<Item = &'a Scene>, thr: f32, rule: Option<(Sig, f64)>) -> Tally {
    let mut t = Tally::default();
    for s in scenes {
        let r = simulate(&s.wins, thr, rule);
        if s.tail {
            t.tails += 1;
            if r.is_some() {
                t.let_through += 1;
            } else {
                t.blocked += 1;
            }
            continue;
        }
        if r.is_some_and(|(_, i)| s.wins[i].end() <= s.by_on) {
            t.early += 1;
        }
        let base = simulate(&s.wins, thr, None);
        let Some((_, bi)) = base else {
            t.base_missed += 1;
            continue;
        };
        t.base_kept += 1;
        match r {
            Some((_, ri)) => {
                t.kept += 1;
                if ri > bi {
                    t.delayed += 1;
                    t.delays.push(s.wins[ri].end() - s.wins[bi].end());
                }
            }
            None => t.lost += 1,
        }
    }
    t
}

fn pct(v: &mut [usize], q: f64) -> usize {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn fpct(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn row(name: &str, t: f64, x: &Tally) -> String {
    let mut d = x.delays.clone();
    format!(
        "| {name} | {t:.3} | {} | {} | {} | {} | {} / {} | {} | {} / {} | {} | {} |\n",
        x.base_kept,
        x.kept,
        x.lost,
        x.delayed,
        pct(&mut d, 0.5),
        pct(&mut d, 1.0),
        x.early,
        x.blocked,
        x.tails,
        x.let_through,
        x.base_missed,
    )
}

const HEADER: &str = "| signal | threshold | takeovers today kept | kept | lost | delayed | delay ms p50 / max | early (bed split) | tails blocked / of | tails let through | takeovers today missed |\n|---|---|---|---|---|---|---|---|---|---|---|\n";

fn run() -> Result<(), String> {
    let mut fixtures = None;
    let mut model = None;
    let mut thr = 0.459f32;
    let mut out: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match arg.as_str() {
            "--fixtures" => fixtures = Some(PathBuf::from(value("--fixtures")?)),
            "--model" => model = Some(PathBuf::from(value("--model")?)),
            "--threshold" => {
                thr = value("--threshold")?
                    .parse()
                    .map_err(|e| format!("--threshold: {e}"))?
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let fixtures = fixtures.ok_or("--fixtures is required")?;
    let embedder = Embedder::open(&model.ok_or("--model is required")?)?;
    let manifest = parse_json(
        &fs::read_to_string(fixtures.join("manifest.json"))
            .map_err(|e| format!("manifest: {e}"))?,
    )?;
    let all = match manifest.get("scenes") {
        Some(Json::Arr(a)) => a,
        _ => return Err("manifest has no scenes".into()),
    };
    let mut costs = Costs {
        embed_s: Vec::new(),
        energy_s: Vec::new(),
    };
    let mut scenes = Vec::new();
    // [takeover, tail] x [ok, no vad, separate, first other, short]
    let mut outcomes = [[0usize; 5]; 2];
    for scene in all {
        let kind = text(scene, "kind");
        if !matches!(kind.as_deref(), Some("caller_to_other" | "caller_tail")) {
            continue;
        }
        let (_, tail, st) = analyze(&fixtures, scene, &embedder, thr, &mut costs)?;
        let o = &mut outcomes[usize::from(tail)];
        match st {
            Status::Ok(s) => {
                o[0] += 1;
                scenes.push(*s);
            }
            Status::NoVad => o[1] += 1,
            Status::Separate => o[2] += 1,
            Status::FirstOther => o[3] += 1,
            Status::Short => o[4] += 1,
        }
    }
    let sigs = [
        Sig::Energy(0),
        Sig::Energy(1),
        Sig::Energy(2),
        Sig::Vad,
        Sig::Score,
    ];
    let main = || scenes.iter().filter(|s| !s.excluded);
    let mut md = String::new();
    md.push_str(&format!(
        "Scenes: {}. Threshold {thr}, 1500 ms windows + 300 ms pre-roll, 2-window caller→other confirm, {MAX_COUNTED} counted windows.\n\n",
        fixtures.display()
    ));
    md.push_str("### Scene outcomes\n\n| set | scored (window 1 caller) | no VAD utterance on the caller | separate (VAD closed before the bystander) | window 1 other | no full window |\n|---|---|---|---|---|---|\n");
    for (i, name) in ["takeover", "tail"].iter().enumerate() {
        let o = outcomes[i];
        md.push_str(&format!(
            "| {name} | {} | {} | {} | {} | {} |\n",
            o[0], o[1], o[2], o[3], o[4]
        ));
    }
    // Signal distributions: today's confirming takeover windows vs tail `other` windows.
    md.push_str("\n### Signal per window: takeover confirming windows vs tail `other` windows\n\n");
    md.push_str("Takeover: the two windows that confirm today's change (main pairs, both windows overlap or follow the bystander's onset). Tail: every `other` window after the caller label.\n\n");
    md.push_str("| signal | takeover min / p05 / p50 | tail p50 / p95 / max | margin (takeover min - tail max) |\n|---|---|---|---|\n");
    let mut margins = BTreeMap::new();
    for s in sigs {
        let (mut tk, mut tl) = (Vec::new(), Vec::new());
        for sc in main() {
            let lv = levels(&sc.wins, thr);
            if sc.tail {
                for (i, w) in sc.wins.iter().enumerate().skip(1) {
                    if w.score < thr {
                        tl.push(sig_value(s, w, lv[i]));
                    }
                }
            } else if let Some((f, c)) = simulate(&sc.wins, thr, None) {
                for i in [f, c] {
                    if sc.wins[i].end() > sc.by_on {
                        tk.push(sig_value(s, &sc.wins[i], lv[i]));
                    }
                }
            }
        }
        let tk_min = fpct(&mut tk, 0.0);
        let tl_max = fpct(&mut tl, 1.0);
        let margin = if s == Sig::Score {
            // Score: takeover windows must score below the tail windows.
            fpct(&mut tl, 0.0) - fpct(&mut tk, 1.0)
        } else {
            tk_min - tl_max
        };
        margins.insert(sig_name(s), margin);
        md.push_str(&format!(
            "| {} | {:.3} / {:.3} / {:.3} | {:.3} / {:.3} / {:.3} | {margin:+.3}{} |\n",
            sig_name(s),
            tk_min,
            fpct(&mut tk, 0.05),
            fpct(&mut tk, 0.5),
            fpct(&mut tl, 0.5),
            fpct(&mut tl, 0.95),
            tl_max,
            if s == Sig::Score {
                " (tail min - takeover max)"
            } else {
                ""
            }
        ));
    }
    md.push_str("\n### Candidates (main pairs; amy-low ← libritts_r 120 excluded)\n\n");
    md.push_str(HEADER);
    let mut passing = Vec::new();
    let mut ranked = Vec::new();
    for s in sigs {
        for t in sig_thresholds(s) {
            let x = tally(main(), thr, Some((s, t)));
            md.push_str(&row(&sig_name(s), t, &x));
            if x.lost == 0 && x.let_through == 0 && x.early == 0 {
                passing.push((s, t));
            }
            ranked.push((x.lost + x.let_through + x.early, s, t));
        }
    }
    md.push_str("\nThreshold 0 (score: 0.459) is today's gate.\n");
    // Breakdowns for the passing (or the two closest) candidates plus today's gate.
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.total_cmp(&a.2)));
    let picks: Vec<(Sig, f64)> = if passing.is_empty() {
        ranked.iter().take(2).map(|r| (r.1, r.2)).collect()
    } else {
        let mut p = Vec::new();
        for s in sigs {
            // The middle passing threshold of each signal (widest margin either side).
            let ts: Vec<f64> = passing.iter().filter(|x| x.0 == s).map(|x| x.1).collect();
            if !ts.is_empty() {
                p.push((s, ts[ts.len() / 2]));
            }
        }
        p
    };
    md.push_str(&format!(
        "\n### Bar (0 lost, 0 early, all tails blocked): {} passing (signal, threshold)\n\n",
        passing.len()
    ));
    for s in sigs {
        let ts: Vec<String> = passing
            .iter()
            .filter(|x| x.0 == s)
            .map(|x| format!("{:.2}", x.1))
            .collect();
        md.push_str(&format!(
            "- {}: {}\n",
            sig_name(s),
            if ts.is_empty() {
                "none".into()
            } else {
                ts.join(", ")
            }
        ));
    }
    for (s, t) in std::iter::once((Sig::Energy(0), 0.0)).chain(picks.iter().copied()) {
        let label = if t == 0.0 {
            "today's gate".to_string()
        } else {
            format!("{} @ {t:.2}", sig_name(s))
        };
        md.push_str(&format!("\n#### {label}: by bystander level\n\n"));
        md.push_str(HEADER);
        let mut lv: Vec<i64> = main().filter(|x| !x.tail).map(|x| x.by_db).collect();
        lv.sort_unstable();
        lv.dedup();
        for l in lv {
            let x = tally(
                main().filter(|x| !x.tail && x.by_db == l),
                thr,
                Some((s, t)),
            );
            md.push_str(&row(&format!("{l} dB"), t, &x));
        }
        md.push_str(&format!("\n#### {label}: by voice pair\n\n"));
        md.push_str(HEADER);
        let mut pairs: Vec<&String> = scenes.iter().filter(|x| !x.tail).map(|x| &x.pair).collect();
        pairs.sort();
        pairs.dedup();
        for p in pairs {
            let x = tally(
                scenes.iter().filter(|x| !x.tail && &x.pair == p),
                thr,
                Some((s, t)),
            );
            let tag = if scenes.iter().any(|x| &x.pair == p && x.excluded) {
                " (excluded, FAR >= 5 %)"
            } else {
                ""
            };
            md.push_str(&row(&format!("{p}{tag}"), t, &x));
        }
    }
    md.push_str(&cut_section(&scenes));
    let mut e = costs.embed_s.iter().map(|s| s * 1e3).collect::<Vec<_>>();
    let mut g = costs.energy_s.iter().map(|s| s * 1e6).collect::<Vec<_>>();
    md.push_str(&format!(
        "\n### Per-window cost\n\nWindows scored: {}. Embed ms p50 / p99 / max: {:.1} / {:.1} / {:.1}. Energy share (frame dB + p90 + share) µs p50 / p99 / max: {:.1} / {:.1} / {:.1}, {:.4} % of the {VERIFY_MS} ms verify budget at p99. The VAD-active share reuses the flag the agent's VAD already computes per frame (no extra work, but `agent.rs` must pass it with each frame).\n",
        e.len(),
        fpct(&mut e, 0.5),
        fpct(&mut e, 0.99),
        fpct(&mut e, 1.0),
        fpct(&mut g, 0.5),
        fpct(&mut g, 0.99),
        fpct(&mut g, 1.0),
        fpct(&mut g, 0.99) / 1e3 / VERIFY_MS * 100.0,
    ));
    let _ = margins;
    print!("{md}");
    if let Some(path) = out {
        fs::write(&path, &md).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Chunk 6e: caller -> other cut candidates on today's confirmed changes (main pairs).
fn cut_section(scenes: &[Scene]) -> String {
    let mut md = String::from(
        "\n### Cut point (chunk 6e): caller → other cut candidates on today's confirmed changes\n\n\
         Main pairs, every scene where today's gate confirms a caller → other change (tails: the false change on bed; takeovers: the real one). \
         Caller words cut = caller words whose midpoint is at or after the cut (they move into the `other` part); \
         bystander words left = bystander words whose midpoint is before the cut (they stay in the caller part). \
         Word times are uniform over the voiced span, as in `split_point_bench`. \
         `cut - caller end` = cut minus the caller's voiced end, ms.\n\n\
         | candidate | tails: caller words cut (scenes) / of | tail cut - caller end p00 / p05 / p50 | takeovers: caller words cut (scenes) / of | takeovers: bystander words left (scenes) | takeover cut - caller end p00 / p05 / p50 / p95 | takeover cut - bystander on p50 / p95 / max | cost µs p99 / max | run-start branch: tails / takeovers |\n\
         |---|---|---|---|---|---|---|---|---|\n",
    );
    let main: Vec<&Scene> = scenes.iter().filter(|s| !s.excluded).collect();
    for (ci, c) in CUTS.iter().enumerate() {
        let (mut tw, mut ts, mut tn) = (0, 0, 0);
        let (mut to, mut ko) = (0, 0);
        let (mut kw, mut ks, mut kn, mut lw, mut ls) = (0, 0, 0, 0, 0);
        let (mut td, mut kd, mut kb, mut cost) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for s in &main {
            let Some(cuts) = &s.cuts else { continue };
            let at = cuts.at[ci];
            cost.push(cuts.secs[ci] * 1e6);
            if cuts.open[ci] {
                if s.tail {
                    to += 1;
                } else {
                    ko += 1;
                }
            }
            let cut = s.caller.2 - words_before(s.caller, at);
            let d = at as f64 - s.caller.1 as f64;
            if s.tail {
                tn += 1;
                tw += cut;
                ts += usize::from(cut > 0);
                td.push(d);
            } else {
                kn += 1;
                kw += cut;
                ks += usize::from(cut > 0);
                kd.push(d);
                if let Some(b) = s.by {
                    let left = words_before(b, at);
                    lw += left;
                    ls += usize::from(left > 0);
                    kb.push(at as f64 - b.0 as f64);
                }
            }
        }
        md.push_str(&format!(
            "| {} | {tw} ({ts}) / {tn} | {:.0} / {:.0} / {:.0} | {kw} ({ks}) / {kn} | {lw} ({ls}) | {:.0} / {:.0} / {:.0} / {:.0} | {:.0} / {:.0} / {:.0} | {:.1} / {:.1} | {to} / {ko} |\n",
            cut_name(*c),
            fpct(&mut td, 0.0),
            fpct(&mut td, 0.05),
            fpct(&mut td, 0.5),
            fpct(&mut kd, 0.0),
            fpct(&mut kd, 0.05),
            fpct(&mut kd, 0.5),
            fpct(&mut kd, 0.95),
            fpct(&mut kb, 0.5),
            fpct(&mut kb, 0.95),
            fpct(&mut kb, 1.0),
            fpct(&mut cost, 0.99),
            fpct(&mut cost, 1.0),
        ));
    }
    md.push_str(&format!(
        "\nShip bar (decision q): 0 caller words cut on tails, takeovers no worse than today on caller words cut and bystander words left, inside the {VERIFY_MS} ms verify budget.\n"
    ));
    md
}

fn main() {
    if let Err(e) = run() {
        eprintln!("caller_to_other_bench: {e}");
        std::process::exit(1);
    }
}
