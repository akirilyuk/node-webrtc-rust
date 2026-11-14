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

struct Scene {
    tail: bool,
    by_db: i64,
    pair: String,
    excluded: bool,
    by_on: usize,
    wins: Vec<Win>,
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
    let by_seg = segs.iter().find(|s| role(s) == "bystander");
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

fn main() {
    if let Err(e) = run() {
        eprintln!("caller_to_other_bench: {e}");
        std::process::exit(1);
    }
}
