//! Split-point bench for the speaker gate's `other -> caller` split (plan
//! `2026-11-13-speaker-split-other-to-caller`, decision c): where in a merged utterance
//! "bystander, bed, caller" should the split go, and do the two search stages find it?
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
//!   --models <speaker dir> --main-tts <amy-low dir> --phrases <stt-phrase-qualification.json> \
//!   --out <dir> --other-to-caller 50
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example split_point_bench -- \
//!   --fixtures <dir> --model <nemo_en_titanet_small.onnx> [--threshold 0.459] [--out <file.md>]
//! ```
//!
//! Per S4 scene (`kind: other_to_caller`, see `speaker_fixtures`):
//! 1. The 48 kHz scene runs through the speech crate's `VadEngine` with the airport demo's VAD
//!    ([`demo_vad_config`]: adaptive, threshold 0.05, 200 ms min speech, 1300 ms min silence). The
//!    VAD utterance that holds the caller's voiced onset is the one the gate scores; when it opens
//!    on the caller (no bystander speech in it) the scene needs no split and is counted as
//!    `separate`.
//! 2. The gate's window grid: window `k` (1-based) scores `[U + (k-1) * 1500, U + k * 1500)` ms,
//!    `U` = the utterance start (SpeechStart minus the minimum speech duration), with 300 ms of
//!    pre-roll in the clip. Each window is embedded (TitaNet) and scored by cosine against the
//!    reference, embedded from the scene's enrollment sentence; `caller` = score >= threshold.
//!    The change is the first `caller` window `k >= 2` after an `other` window (1-window confirm,
//!    decision d). No such window: `first_caller` (window 1 already caller) or `missed`.
//! 3. Search range (decision c): `[start(k-1), start(k) + SPLIT_SEARCH_MS]`.
//!    - Stage 1: 20 ms frame energies, smoothed over 5 frames (100 ms, power mean). A frame is
//!      quiet below the midpoint (dB) of the range's 10th and 90th percentile. The split is the end
//!      of the longest quiet run (ties: the later run); it is used when the run is at least the
//!      quiet-run minimum.
//!    - Stage 2: sub-windows of `L` ms on an `H` ms hop from `start(k-1)` to `start(k) + 1500`
//!      (the two clips the Watch holds), each embedded and scored. The localized point is the
//!      start (`start`), centre plus a margin (`mid+M`) of the first sub-window scoring `caller`,
//!      or the end of the sub-window before it (`last-other-end`), refined to the lowest-energy 20 ms frame within +-`R` ms. No sub-window `caller`: stage 1's
//!      point regardless of the minimum.
//! 4. Measured against the manifest: distance = split - caller voiced onset (ms; negative = before
//!    the caller). Words are placed linearly over each segment's voiced span
//!    (`voiced_start_ms`..`voiced_end_ms`); a caller word is **cut** when its midpoint is before the
//!    split, a bystander word **leaks** when its midpoint is at or after the split.
//!
//! Output: markdown tables (stdout, and `--out`): scene outcomes, stage 1 quiet runs, stage 2 per
//! variant alone, the combined pipeline per quiet-run minimum and variant, and the stage 2 embed
//! times.

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
/// The gate's check window (`check_speech_ms` default) and pre-roll.
const WINDOW_MS: usize = 1500;
const PRE_ROLL_MS: usize = 300;
/// The gate's `SPLIT_SEARCH_MS`.
const SPLIT_SEARCH_MS: usize = 300;
/// The gate's `recheck_max_windows` default.
const MAX_WINDOWS: usize = 12;
const FRAME_MS: usize = 20;
const SMOOTH_FRAMES: usize = 5;
/// Stage 2 variants: (sub-window ms, hop ms).
const SUB_WINDOWS: [(usize, usize); 4] = [(750, 250), (1000, 250), (1000, 125), (1250, 250)];
const REFINE_MS: [usize; 2] = [100, 150];
const QUIET_MINS: [usize; 4] = [500, 800, 1000, 1500];

/// Stage 2 localized point, from the first sub-window scoring `caller` (index `i`).
#[derive(Clone, Copy)]
enum Anchor {
    /// Its start.
    Start,
    /// Its centre plus this many ms.
    Mid(usize),
    /// The end of sub-window `i - 1` (the last `other` one before it); its start when `i == 0`.
    LastOtherEnd,
}

const ANCHORS: [Anchor; 6] = [
    Anchor::Start,
    Anchor::Mid(0),
    Anchor::Mid(100),
    Anchor::Mid(200),
    Anchor::Mid(300),
    Anchor::LastOtherEnd,
];

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

    /// Embedding of `samples` and the seconds `compute` took (stream, waveform, embedding).
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

/// Power (mean square) of the 20 ms frame starting at `t_ms`.
fn frame_power(samples: &[f32], t_ms: usize) -> f64 {
    let f = slice(samples, t_ms, t_ms + FRAME_MS);
    if f.is_empty() {
        return 0.0;
    }
    f.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / f.len() as f64
}

fn db(power: f64) -> f64 {
    10.0 * power.max(1e-12).log10()
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i]
}

/// Stage 1 over `[lo, hi)`: (split ms, longest quiet run ms). See the module docs.
fn stage1(samples: &[f32], lo: usize, hi: usize) -> (usize, usize) {
    let n = (hi.saturating_sub(lo)) / FRAME_MS;
    let raw: Vec<f64> = (0..n)
        .map(|i| frame_power(samples, lo + i * FRAME_MS))
        .collect();
    let smooth: Vec<f64> = (0..n)
        .map(|i| {
            let a = i.saturating_sub(SMOOTH_FRAMES / 2);
            let b = (i + SMOOTH_FRAMES / 2 + 1).min(n);
            db(raw[a..b].iter().sum::<f64>() / (b - a) as f64)
        })
        .collect();
    let mut sorted = smooth.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = (percentile(&sorted, 0.1) + percentile(&sorted, 0.9)) / 2.0;
    let (mut best_len, mut best_end, mut run) = (0usize, lo, 0usize);
    for (i, v) in smooth.iter().enumerate() {
        if *v < mid {
            run += 1;
            if run >= best_len {
                best_len = run;
                best_end = lo + (i + 1) * FRAME_MS;
            }
        } else {
            run = 0;
        }
    }
    (best_end, best_len * FRAME_MS)
}

/// Lowest-energy 20 ms frame start within `+-r` ms of `p` (ties: nearest `p`, then earlier).
fn refine(samples: &[f32], p: usize, r: usize) -> usize {
    let mut best: Option<(f64, usize, usize)> = None;
    let mut t = p.saturating_sub(r);
    while t <= p + r {
        let cand = (frame_power(samples, t), t.abs_diff(p), t);
        if best.is_none_or(|b| cand.partial_cmp(&b) == Some(std::cmp::Ordering::Less)) {
            best = Some(cand);
        }
        t += FRAME_MS;
    }
    best.map_or(p, |b| b.2)
}

#[derive(Clone, Copy)]
struct Span {
    on: usize,
    off: usize,
    words: usize,
}

impl Span {
    /// Words of this span whose midpoint is before `t`.
    fn words_before(&self, t: usize) -> usize {
        let d = self.off.saturating_sub(self.on) as f64;
        (0..self.words)
            .filter(|j| (self.on as f64 + (*j as f64 + 0.5) * d / self.words as f64) < t as f64)
            .count()
    }
}

/// Stage 2 sub-window scores of one scene for one (L, H): (start ms, score, embed s).
struct SubScores {
    subs: Vec<(usize, f32, f64)>,
}

struct Case {
    id: String,
    paused: bool,
    samples: Vec<f32>,
    caller: Span,
    by: Span,
    lo: usize,
    hi: usize,
    window_start: usize,
    s1_split: usize,
    s1_run: usize,
    stage2: Vec<SubScores>,
}

enum Outcome {
    Change(Box<Case>),
    Separate,
    FirstCaller,
    /// Merged, but no `caller` window after an `other` one (the utterance ends first).
    Missed,
    /// The VAD never opens on or after the caller's onset.
    NoVad,
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

fn span_of(seg: &Json) -> Option<Span> {
    Some(Span {
        on: num(seg, "voiced_start_ms")? as usize,
        off: num(seg, "voiced_end_ms")? as usize,
        words: text(seg, "text")?.split_whitespace().count().max(1),
    })
}

/// The VAD of the airport demo and the Sherpa roundtrips (`resolve-voice-config.ts` with
/// `provider: 'adaptive'`): threshold 0.05 (adaptive warm-up), 200 ms minimum speech, 1300 ms
/// minimum silence; everything else at its default.
fn demo_vad_config() -> VadConfig {
    VadConfig {
        provider: "adaptive".into(),
        threshold: 0.05,
        min_speech_duration_ms: 200,
        min_silence_duration_ms: 1300,
        ..VadConfig::default()
    }
}

/// First VAD utterance `[start_ms, end_ms)` that ends after `t` (start = SpeechStart - min speech duration).
fn vad_utterance(pcm48: &[u8], t: usize) -> Result<Option<(usize, usize)>, String> {
    let config = demo_vad_config();
    let mut vad = VadEngine::new(config.clone()).map_err(|e| e.to_string())?;
    let frame_bytes = 960 * 2 * 2;
    let mut start = None;
    let mut utts = Vec::new();
    let mut total = 0;
    for (i, frame) in pcm48.chunks_exact(frame_bytes).enumerate() {
        let ms = i * 20;
        total = ms + 20;
        let (transitions, _) = vad
            .process_webrtc_pcm(frame, 20)
            .map_err(|e| e.to_string())?;
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
        utts.push((s, total));
    }
    Ok(utts.into_iter().find(|(_, e)| t < *e))
}

fn analyze(
    fixtures: &Path,
    scene: &Json,
    embedder: &Embedder,
    threshold: f32,
) -> Result<(String, bool, Outcome), String> {
    let id = text(scene, "id").ok_or("scene id")?;
    let paused = id.starts_with("s4_test_p");
    let segs = match scene.get("segments") {
        Some(Json::Arr(a)) => a,
        _ => return Err(format!("{id}: no segments")),
    };
    let role = |s: &Json| text(s, "role").unwrap_or_default();
    let enroll_seg = segs
        .iter()
        .find(|s| matches!(s.get("enroll"), Some(Json::Bool(true))))
        .ok_or(format!("{id}: no enrollment"))?;
    let by_seg = segs
        .iter()
        .find(|s| role(s) == "bystander")
        .ok_or(format!("{id}: no bystander"))?;
    let caller_seg = segs
        .iter()
        .find(|s| role(s) == "main" && !matches!(s.get("enroll"), Some(Json::Bool(true))))
        .ok_or(format!("{id}: no caller"))?;
    let (enroll, by, caller) = (
        span_of(enroll_seg).ok_or("enroll span")?,
        span_of(by_seg).ok_or("bystander span")?,
        span_of(caller_seg).ok_or("caller span")?,
    );
    let samples = read_wav(&fixtures.join("scenes").join(format!("{id}.16k.wav")))?;
    let pcm48 = read_wav_stereo_48k_pcm(&fixtures.join("scenes").join(format!("{id}.wav")))?;
    // The utterance the caller's speech is in: the first one still open after the caller's
    // -45 dBFS onset (the VAD can open a little, or after a long pause well, after it).
    let Some((u, u_end)) = vad_utterance(&pcm48, caller.on)? else {
        return Ok((id, paused, Outcome::NoVad));
    };
    if u + 200 >= caller.on || u >= by.off {
        return Ok((id, paused, Outcome::Separate));
    }
    let (reference, _) = embedder
        .embed(slice(&samples, enroll.on, enroll.off))
        .ok_or("enroll embed")?;
    let score = |a: usize, b: usize| -> f32 {
        embedder
            .embed(slice(&samples, a, b))
            .map_or(0.0, |(e, _)| cosine(&e, &reference))
    };
    let mut labels = Vec::new();
    let mut k = 1;
    while k <= MAX_WINDOWS && u + k * WINDOW_MS <= u_end {
        let start = u + (k - 1) * WINDOW_MS;
        let s = score(start.saturating_sub(PRE_ROLL_MS), start + WINDOW_MS);
        labels.push((start, s));
        k += 1;
    }
    eprintln!(
        "{id}: utt {u}-{u_end} by {}-{} caller {}-{} windows {:?}",
        by.on,
        by.off,
        caller.on,
        caller.off,
        labels
            .iter()
            .map(|(s, v)| format!("{s}:{v:.3}"))
            .collect::<Vec<_>>()
    );
    if labels.first().is_some_and(|(_, s)| *s >= threshold) {
        return Ok((id, paused, Outcome::FirstCaller));
    }
    let Some(k) =
        (1..labels.len()).find(|&i| labels[i].1 >= threshold && labels[i - 1].1 < threshold)
    else {
        return Ok((id, paused, Outcome::Missed));
    };
    let (lo, window_start) = (labels[k - 1].0, labels[k].0);
    let hi = window_start + SPLIT_SEARCH_MS;
    let (s1_split, s1_run) = stage1(&samples, lo, hi);
    let audio_end = window_start + WINDOW_MS;
    let stage2 = SUB_WINDOWS
        .iter()
        .map(|&(l, h)| {
            let mut subs = Vec::new();
            let mut t = lo;
            while t + l <= audio_end {
                let (s, secs) = embedder
                    .embed(slice(&samples, t, t + l))
                    .map_or((0.0, 0.0), |(e, secs)| (cosine(&e, &reference), secs));
                subs.push((t, s, secs));
                t += h;
            }
            SubScores { subs }
        })
        .collect();
    Ok((
        id.clone(),
        paused,
        Outcome::Change(Box::new(Case {
            id,
            paused,
            samples,
            caller,
            by,
            lo,
            hi,
            window_start,
            s1_split,
            s1_run,
            stage2,
        })),
    ))
}

/// Stage 2 split for variant `v` with anchor `mid` and refine `r`; `None` when no sub-window is
/// `caller`.
fn stage2_split(c: &Case, v: usize, anchor: Anchor, r: usize, threshold: f32) -> Option<usize> {
    let (l, _) = SUB_WINDOWS[v];
    let subs = &c.stage2[v].subs;
    let i = subs.iter().position(|(_, s, _)| *s >= threshold)?;
    let t = subs[i].0;
    let p = match anchor {
        Anchor::Start => t,
        Anchor::Mid(m) => t + l / 2 + m,
        Anchor::LastOtherEnd if i > 0 => subs[i - 1].0 + l,
        Anchor::LastOtherEnd => t,
    };
    Some(refine(&c.samples, p, r))
}

#[derive(Default)]
struct Tally {
    dist: Vec<f64>,
    cut_words: usize,
    cut_scenes: usize,
    leak_words: usize,
    leak_scenes: usize,
    n: usize,
}

impl Tally {
    fn add(&mut self, c: &Case, split: usize) {
        self.n += 1;
        self.dist.push(split as f64 - c.caller.on as f64);
        let cut = c.caller.words_before(split);
        let leak = c.by.words - c.by.words_before(split);
        self.cut_words += cut;
        self.cut_scenes += usize::from(cut > 0);
        self.leak_words += leak;
        self.leak_scenes += usize::from(leak > 0);
    }

    fn dist_cell(&self) -> String {
        let mut d = self.dist.clone();
        d.sort_by(|a, b| a.total_cmp(b));
        if d.is_empty() {
            return "-".into();
        }
        format!(
            "{:.0} / {:.0} / {:.0} / {:.0}",
            d[0],
            percentile(&d, 0.5),
            percentile(&d, 0.95),
            d[d.len() - 1]
        )
    }

    fn row(&self) -> String {
        format!(
            "{} | {} | {} ({}) | {} ({})",
            self.n,
            self.dist_cell(),
            self.cut_words,
            self.cut_scenes,
            self.leak_words,
            self.leak_scenes
        )
    }
}

fn variant_name(v: usize, anchor: Anchor, r: usize) -> String {
    let (l, h) = SUB_WINDOWS[v];
    let a = match anchor {
        Anchor::Start => "start".to_string(),
        Anchor::Mid(m) => format!("mid+{m}"),
        Anchor::LastOtherEnd => "last-other-end".to_string(),
    };
    format!("L{l}/H{h} {a} ±{r}")
}

fn run() -> Result<(), String> {
    let mut fixtures = None;
    let mut model = None;
    let mut threshold = 0.459f32;
    let mut out: Option<PathBuf> = None;
    let mut bed = String::from("babble (test-set voices)");
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match arg.as_str() {
            "--fixtures" => fixtures = Some(PathBuf::from(value("--fixtures")?)),
            "--model" => model = Some(PathBuf::from(value("--model")?)),
            "--threshold" => {
                threshold = value("--threshold")?
                    .parse()
                    .map_err(|e| format!("--threshold: {e}"))?
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--bed" => bed = value("--bed")?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let fixtures = fixtures.ok_or("--fixtures is required")?;
    let embedder = Embedder::open(&model.ok_or("--model is required")?)?;
    let manifest = parse_json(
        &fs::read_to_string(fixtures.join("manifest.json"))
            .map_err(|e| format!("manifest: {e}"))?,
    )?;
    let scenes = match manifest.get("scenes") {
        Some(Json::Arr(a)) => a,
        _ => return Err("manifest has no scenes".into()),
    };
    let mut cases = Vec::new();
    // (paused, separate, first_caller, missed) counts per set.
    let mut outcomes = [[0usize; 5]; 2];
    for scene in scenes {
        if text(scene, "kind").as_deref() != Some("other_to_caller") {
            continue;
        }
        let (id, paused, outcome) = analyze(&fixtures, scene, &embedder, threshold)?;
        let set = usize::from(!paused);
        match outcome {
            Outcome::Change(c) => {
                outcomes[set][0] += 1;
                cases.push(*c);
            }
            Outcome::Separate => outcomes[set][1] += 1,
            Outcome::FirstCaller => outcomes[set][2] += 1,
            Outcome::Missed => outcomes[set][3] += 1,
            Outcome::NoVad => outcomes[set][4] += 1,
        }
        eprintln!("{id} done");
    }
    let sets = [("paused", true), ("no pause", false)];
    let mut md = String::new();
    let bed_levels: std::collections::BTreeSet<i64> = scenes
        .iter()
        .filter(|s| text(s, "kind").as_deref() == Some("other_to_caller"))
        .filter_map(|s| num(s, "babble_db").map(|d| d as i64))
        .collect();
    md.push_str(&format!(
        "Bed: {bed} at {bed_levels:?} dB re the main voice. Scenes: {}.\n\n",
        fixtures.display()
    ));
    md.push_str(&format!(
        "Threshold {threshold}. Distance = split - caller voiced onset (ms) min / p50 / p95 / max. \
         cut = caller words before the split (scenes), leak = bystander words at or after the split (scenes).\n\n"
    ));
    md.push_str("### Scene outcomes\n\n| set | other→caller change | separate utterances (VAD split them) | window 1 already caller | merged, no caller window before the close | no VAD utterance on the caller |\n|---|---|---|---|---|---|\n");
    for (i, (name, _)) in sets.iter().enumerate() {
        let o = outcomes[i];
        md.push_str(&format!(
            "| {name} | {} | {} | {} | {} | {} |\n",
            o[0], o[1], o[2], o[3], o[4]
        ));
    }
    md.push_str("\n### Stage 1 alone (longest quiet run, any length)\n\n| set | quiet run ms min / p50 / p95 / max | n | distance | cut | leak |\n|---|---|---|---|---|---|\n");
    for (name, paused) in sets {
        let mut t = Tally::default();
        let mut runs: Vec<f64> = Vec::new();
        for c in cases.iter().filter(|c| c.paused == paused) {
            t.add(c, c.s1_split);
            runs.push(c.s1_run as f64);
        }
        runs.sort_by(|a, b| a.total_cmp(b));
        let r = if runs.is_empty() {
            "-".to_string()
        } else {
            format!(
                "{:.0} / {:.0} / {:.0} / {:.0}",
                runs[0],
                percentile(&runs, 0.5),
                percentile(&runs, 0.95),
                runs[runs.len() - 1]
            )
        };
        md.push_str(&format!("| {name} | {r} | {} |\n", t.row()));
    }
    md.push_str("\n### Stage 2 alone\n\n| variant | set | n | distance | cut | leak | no caller sub-window |\n|---|---|---|---|---|---|---|\n");
    let variants: Vec<(usize, Anchor, usize)> = (0..SUB_WINDOWS.len())
        .flat_map(|v| ANCHORS.map(move |m| (v, m)))
        .flat_map(|(v, m)| REFINE_MS.map(move |r| (v, m, r)))
        .collect();
    for &(v, anchor, r) in &variants {
        for (name, paused) in sets {
            let mut t = Tally::default();
            let mut none = 0;
            for c in cases.iter().filter(|c| c.paused == paused) {
                match stage2_split(c, v, anchor, r, threshold) {
                    Some(s) => t.add(c, s),
                    None => {
                        none += 1;
                        t.add(c, c.s1_split);
                    }
                }
            }
            md.push_str(&format!(
                "| {} | {name} | {} | {none} |\n",
                variant_name(v, anchor, r),
                t.row()
            ));
        }
    }
    md.push_str("\n### Combined (stage 1 when its quiet run >= minimum, else stage 2)\n\n| quiet min ms | variant | paused: n, distance, cut, leak | stage 2 used | no pause: n, distance, cut, leak | stage 2 used |\n|---|---|---|---|---|---|\n");
    for qm in QUIET_MINS {
        for &(v, anchor, r) in &variants {
            let mut cells = Vec::new();
            for (_, paused) in sets {
                let mut t = Tally::default();
                let mut used = 0;
                for c in cases.iter().filter(|c| c.paused == paused) {
                    let split = if c.s1_run >= qm {
                        c.s1_split
                    } else {
                        used += 1;
                        stage2_split(c, v, anchor, r, threshold).unwrap_or(c.s1_split)
                    };
                    t.add(c, split);
                }
                cells.push(format!("{} | {used}", t.row().replace(" | ", ", ")));
            }
            md.push_str(&format!(
                "| {qm} | {} | {} | {} |\n",
                variant_name(v, anchor, r),
                cells[0],
                cells[1]
            ));
        }
    }
    md.push_str("\n### Stage 2 embed time\n\n| sub-window/hop | embeds per scene p50 / max | per embed ms p50 / p99 / max | per scene total ms p50 / p99 / max |\n|---|---|---|---|\n");
    for (v, (l, h)) in SUB_WINDOWS.iter().enumerate() {
        let mut per: Vec<f64> = Vec::new();
        let mut totals: Vec<f64> = Vec::new();
        let mut counts: Vec<f64> = Vec::new();
        for c in &cases {
            let subs = &c.stage2[v].subs;
            per.extend(subs.iter().map(|s| s.2 * 1000.0));
            totals.push(subs.iter().map(|s| s.2 * 1000.0).sum());
            counts.push(subs.len() as f64);
        }
        for x in [&mut per, &mut totals, &mut counts] {
            x.sort_by(|a, b| a.total_cmp(b));
        }
        let last = |x: &Vec<f64>| x.last().copied().unwrap_or(f64::NAN);
        md.push_str(&format!(
            "| {l}/{h} | {:.0} / {:.0} | {:.1} / {:.1} / {:.1} | {:.0} / {:.0} / {:.0} |\n",
            percentile(&counts, 0.5),
            last(&counts),
            percentile(&per, 0.5),
            percentile(&per, 0.99),
            last(&per),
            percentile(&totals, 0.5),
            percentile(&totals, 0.99),
            last(&totals)
        ));
    }
    md.push_str("\n### Per scene (stage 1)\n\n| scene | range | first caller window | bystander voiced | caller voiced | quiet run ms | stage 1 split |\n|---|---|---|---|---|---|---|\n");
    for c in &cases {
        md.push_str(&format!(
            "| {} | {}-{} | {} | {}-{} | {}-{} | {} | {} |\n",
            c.id,
            c.lo,
            c.hi,
            c.window_start,
            c.by.on,
            c.by.off,
            c.caller.on,
            c.caller.off,
            c.s1_run,
            c.s1_split
        ));
    }
    print!("{md}");
    if let Some(path) = out {
        fs::write(&path, &md).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("split_point_bench: {e}");
        std::process::exit(1);
    }
}
