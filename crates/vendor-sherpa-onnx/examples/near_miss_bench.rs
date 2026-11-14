//! Near-miss re-check bench for the speaker gate's `other -> caller` split (plan
//! `2026-11-13-speaker-split-other-to-caller`, chunk 6f, decision t).
//!
//! eu-load run 43585163115 (scene `s4_test_n05`): a short trailing caller phrase (~1.45 s, no
//! pause after the bystander) straddles two 1.5 s windows that score just under the threshold
//! (0.442, 0.426 vs 0.459), so no `caller` window confirms the change and the phrase is lost with
//! the bystander utterance. Decision t: when a window on an `other` utterance scores in a near-miss
//! band `[lo, threshold)`, re-score the Watch's stretch (previous window start .. this window end)
//! with the stage-2 sub-windows (1250 ms on a 250 ms hop); `need` consecutive sub-windows at or
//! above the threshold count as the confirming `caller` window.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
//!   --models <.models/speaker> --main-tts <.models/vits-piper-en_US-amy-low> \
//!   --phrases <stt-phrase-qualification.json> --out /tmp/sg-nm-n400 --other-to-caller 400 --only s4_test_n
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
//!   ... --out /tmp/sg-nm-p200 --other-to-caller 200 --only s4_test_p
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example near_miss_bench -- \
//!   --model <nemo_en_titanet_small.onnx> --fixtures /tmp/sg-nm-n400 --fixtures /tmp/sg-nm-p200 \
//!   [--tap-utt <stt-in-…-stop.wav> --tap-enroll <stt-in-…-utt1.wav> --tap-scores 0.083,0.023,…] \
//!   [--threshold 0.459] [--out <file.md>]
//! ```
//!
//! Per scene (same VAD and window grid as `split_point_bench`: demo adaptive VAD, window `k`
//! scores `[U + (k-1)*1500 - 300, U + k*1500)`, `U` = SpeechStart minus 200 ms):
//! - **Recovery** (no-pause scenes, caller voiced 1000-1600 ms, VAD merged bystander and caller):
//!   today = the first window `k >= 2` scoring `caller`; variant = today, or a near-miss window
//!   whose stretch has `need` consecutive `caller` sub-windows. A confirm whose audio ends before
//!   the caller's voiced onset is a gate false accept (counted apart). Split: stage 1 (longest
//!   quiet run in `[start(k-1), start(k) + 300]`) when the run is at least 500 ms, else the first
//!   `caller` sub-window start moved to the end of the longest quiet run within 500 ms after it
//!   (the product's `other_to_caller_split`). Caller words with their midpoint before the split
//!   are **clipped**; bystander words at or after it **leak**. No confirm: the phrase is **lost**.
//! - **False accepts** (bystander-only utterances: the paused scenes' bystander utterance, VAD
//!   separated from the caller; and the bystander part of every no-pause scene, windows whose
//!   stretch ends before the caller's onset). Today = a window `k >= 2` scoring `caller` after an
//!   `other` window 1; variant adds the near-miss confirms. Pairs at or above 5 % pair FAR in
//!   `FALSE_ACCEPT_BENCH.md` (libritts_r 120) are excluded, as in the roundtrip.
//! - **Cost**: embeds per re-check and their time against the window's `verify_timeout_ms`
//!   budget (500 ms minus the window's own embed, as `stage2_split` bounds it). A re-check that
//!   would run past the budget is cut off there and confirms nothing (counted).

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
const SPLIT_SEARCH_MS: usize = 300;
const MAX_WINDOWS: usize = 12;
const FRAME_MS: usize = 20;
const SMOOTH_FRAMES: usize = 5;
const QUIET_RUN_MIN_MS: usize = 500;
const SUB_MS: usize = 1250;
const HOP_MS: usize = 250;
const FORWARD_MS: usize = 500;
const VERIFY_MS: f64 = 500.0;
const SHORT_MIN_MS: usize = 1000;
const SHORT_MAX_MS: usize = 1600;
const BAND_LOS: [f32; 4] = [0.30, 0.35, 0.40, 0.42];
const NEEDS: [usize; 2] = [1, 2];
const EXCLUDED: [(&str, i64); 1] = [("en_US-libritts_r-medium", 120)];

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

    /// Embedding of `samples` and the milliseconds `compute` took.
    fn embed(&self, samples: &[f32]) -> Option<(Vec<f32>, f64)> {
        let start = Instant::now();
        let stream = self.extractor.create_stream()?;
        stream.accept_waveform(SR, samples);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return None;
        }
        let embedding = self.extractor.compute(&stream)?;
        Some((embedding, start.elapsed().as_secs_f64() * 1000.0))
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

fn slice(samples: &[f32], start_ms: usize, end_ms: usize) -> &[f32] {
    let a = (start_ms * SAMPLES_PER_MS).min(samples.len());
    let b = (end_ms * SAMPLES_PER_MS).min(samples.len()).max(a);
    &samples[a..b]
}

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

/// Longest quiet run over `[lo, hi)`: (its end ms, its length ms). Same rule as
/// `split_point_bench::stage1` (smoothed 20 ms frames, quiet below the p10/p90 midpoint).
fn quiet_run(samples: &[f32], lo: usize, hi: usize) -> (usize, usize) {
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

#[derive(Clone, Copy)]
struct Span {
    on: usize,
    off: usize,
    words: usize,
}

impl Span {
    /// Words whose linear midpoint is before `t`.
    fn words_before(&self, t: usize) -> usize {
        let dur = self.off.saturating_sub(self.on).max(1) as f64;
        (0..self.words)
            .filter(|i| {
                let mid = self.on as f64 + (*i as f64 + 0.5) * dur / self.words as f64;
                mid < t as f64
            })
            .count()
    }
}

fn span_of(seg: &Json) -> Option<Span> {
    Some(Span {
        on: num(seg, "voiced_start_ms")? as usize,
        off: num(seg, "voiced_end_ms")? as usize,
        words: text(seg, "text")?.split_whitespace().count().max(1),
    })
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

/// Every VAD utterance `[start_ms, end_ms)` (start = SpeechStart - min speech duration).
fn vad_utterances(pcm48: &[u8]) -> Result<Vec<(usize, usize)>, String> {
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
    Ok(utts)
}

/// One scored 1.5 s window: (start ms, score, embed ms).
type Win = (usize, f32, f64);

/// Sub-window scores over the stretch of window `k`: `[start(k-1), start(k) + 1500)`.
struct Recheck {
    /// (sub-window start, score, embed ms).
    subs: Vec<(usize, f32, f64)>,
}

impl Recheck {
    /// First sub-window start that begins `need` consecutive `caller` sub-windows, honoring the
    /// time budget `budget_ms` (sub-windows past it are never computed). `Err(())` = the budget ran
    /// out before a confirm.
    fn confirm(&self, threshold: f32, need: usize, budget_ms: f64) -> Result<Option<usize>, ()> {
        let mut spent = 0.0;
        let mut run = 0;
        for (i, (_, s, ms)) in self.subs.iter().enumerate() {
            spent += ms;
            if spent > budget_ms {
                return Err(());
            }
            if *s >= threshold {
                run += 1;
                if run >= need {
                    return Ok(Some(self.subs[i + 1 - need].0));
                }
            } else {
                run = 0;
            }
        }
        Ok(None)
    }

    fn time_ms(&self) -> f64 {
        self.subs.iter().map(|s| s.2).sum()
    }
}

fn score_windows(
    samples: &[f32],
    u: usize,
    u_end: usize,
    embedder: &Embedder,
    reference: &[f32],
) -> Vec<Win> {
    let mut out = Vec::new();
    let mut k = 1;
    while k <= MAX_WINDOWS && u + k * WINDOW_MS <= u_end {
        let start = u + (k - 1) * WINDOW_MS;
        let (s, ms) = embedder
            .embed(slice(samples, start.saturating_sub(PRE_ROLL_MS), start + WINDOW_MS))
            .map_or((0.0, 0.0), |(e, ms)| (cosine(&e, reference), ms));
        out.push((start, s, ms));
        k += 1;
    }
    out
}

fn recheck(
    samples: &[f32],
    lo: usize,
    end: usize,
    embedder: &Embedder,
    reference: &[f32],
) -> Recheck {
    let mut subs = Vec::new();
    let mut t = lo;
    while t + SUB_MS <= end {
        let (s, ms) = embedder
            .embed(slice(samples, t, t + SUB_MS))
            .map_or((0.0, 0.0), |(e, ms)| (cosine(&e, reference), ms));
        subs.push((t, s, ms));
        t += HOP_MS;
    }
    Recheck { subs }
}

/// A gate run over one utterance's windows for one variant (`band_lo = None` = today).
#[derive(Clone, Copy, Debug)]
enum Confirm {
    /// No confirm.
    None,
    /// Window `k` (index) confirmed at `at` (window start, or the first `caller` sub-window).
    Window { k: usize, at: usize, sub: bool },
}

struct Utt {
    wins: Vec<Win>,
    /// Re-check per window index `k >= 1` (stretch `[start(k-1), start(k) + 1500)`).
    rechecks: Vec<Option<Recheck>>,
}

struct Variant {
    band_lo: Option<f32>,
    need: usize,
}

impl Variant {
    fn name(&self) -> String {
        match self.band_lo {
            None => "today (no re-check)".into(),
            Some(lo) => format!("band [{lo:.2}, thr) need {}", self.need),
        }
    }
}

/// (confirm, re-checks run, re-checks cut by the budget). `limit` = the last window index
/// whose stretch may count (bystander-only: the stretch ends before the caller).
fn run_gate(utt: &Utt, v: &Variant, threshold: f32, limit: usize) -> (Confirm, usize, usize) {
    let (mut ran, mut cut) = (0, 0);
    if utt.wins.first().is_none_or(|w| w.1 >= threshold) {
        return (Confirm::None, 0, 0);
    }
    for k in 1..utt.wins.len().min(limit.saturating_add(1)) {
        let (start, s, ms) = utt.wins[k];
        if s >= threshold {
            return (
                Confirm::Window {
                    k,
                    at: start,
                    sub: false,
                },
                ran,
                cut,
            );
        }
        let Some(lo) = v.band_lo else { continue };
        if s < lo {
            continue;
        }
        let Some(rc) = utt.rechecks[k].as_ref() else {
            continue;
        };
        ran += 1;
        match rc.confirm(threshold, v.need, VERIFY_MS - ms) {
            Ok(Some(at)) => {
                return (Confirm::Window { k, at, sub: true }, ran, cut);
            }
            Ok(None) => {}
            Err(()) => cut += 1,
        }
    }
    (Confirm::None, ran, cut)
}

/// The product's other -> caller split for a confirm of window `k` (see the module docs).
fn split_at(
    samples: &[f32],
    utt: &Utt,
    k: usize,
    confirm_at: usize,
    sub: bool,
    threshold: f32,
) -> usize {
    let lo = utt.wins[k - 1].0;
    let ws = utt.wins[k].0;
    let (s1, run) = quiet_run(samples, lo, ws + SPLIT_SEARCH_MS);
    if run >= QUIET_RUN_MIN_MS {
        return s1;
    }
    let p = if sub {
        confirm_at
    } else {
        // Stage 2 over the stretch: the first `caller` sub-window.
        match utt.rechecks[k].as_ref().and_then(|rc| {
            rc.subs
                .iter()
                .find(|(_, s, _)| *s >= threshold)
                .map(|x| x.0)
        }) {
            Some(p) => p,
            None => return s1,
        }
    };
    let (end, run) = quiet_run(samples, p, p + FORWARD_MS);
    if run > 0 {
        end
    } else {
        p
    }
}

struct Scene {
    id: String,
    paused: bool,
    excluded: bool,
    samples: Vec<f32>,
    by: Span,
    caller: Span,
    /// The utterance holding the caller onset, when it also holds bystander speech (merged).
    merged: Option<Utt>,
    /// Bystander-only utterance (VAD separated from the caller and the enrollment).
    by_only: Option<Utt>,
    /// The merged utterance's last window index whose stretch ends before the caller onset.
    by_limit: usize,
    /// Same-call enrollment embedding.
    reference: Vec<f32>,
}

fn build_utt(
    samples: &[f32],
    u: usize,
    u_end: usize,
    embedder: &Embedder,
    reference: &[f32],
) -> Utt {
    let wins = score_windows(samples, u, u_end, embedder, reference);
    let rechecks = (0..wins.len())
        .map(|k| {
            (k >= 1 && wins[k].1 >= BAND_LOS[0]).then(|| {
                recheck(
                    samples,
                    wins[k - 1].0,
                    wins[k].0 + WINDOW_MS,
                    embedder,
                    reference,
                )
            })
        })
        .collect();
    Utt { wins, rechecks }
}

fn load_scene(
    fixtures: &Path,
    scene: &Json,
    embedder: &Embedder,
) -> Result<Option<Scene>, String> {
    let id = text(scene, "id").ok_or("scene id")?;
    let paused = id.starts_with("s4_test_p");
    let segs = match scene.get("segments") {
        Some(Json::Arr(a)) => a,
        _ => return Err(format!("{id}: no segments")),
    };
    let role = |s: &Json| text(s, "role").unwrap_or_default();
    let is_enroll = |s: &Json| matches!(s.get("enroll"), Some(Json::Bool(true)));
    let enroll_seg = segs.iter().find(|s| is_enroll(s)).ok_or("no enrollment")?;
    let by_seg = segs
        .iter()
        .find(|s| role(s) == "bystander")
        .ok_or("no bystander")?;
    let caller_seg = segs
        .iter()
        .find(|s| role(s) == "main" && !is_enroll(s))
        .ok_or("no caller")?;
    let voice = text(by_seg, "voice").unwrap_or_default();
    let sid = num(by_seg, "sid").unwrap_or(0.0) as i64;
    let excluded = EXCLUDED.iter().any(|(v, s)| *v == voice && *s == sid);
    let (enroll, by, caller) = (
        span_of(enroll_seg).ok_or("enroll span")?,
        span_of(by_seg).ok_or("bystander span")?,
        span_of(caller_seg).ok_or("caller span")?,
    );
    let samples = read_wav(&fixtures.join("scenes").join(format!("{id}.16k.wav")))?;
    let pcm48 = read_wav_stereo_48k_pcm(&fixtures.join("scenes").join(format!("{id}.wav")))?;
    let utts = vad_utterances(&pcm48)?;
    let (reference, _) = embedder
        .embed(slice(&samples, enroll.on, enroll.off))
        .ok_or("enroll embed")?;
    let mut merged = None;
    let mut by_limit = 0;
    if let Some(&(u, u_end)) = utts.iter().find(|(_, e)| caller.on < *e) {
        if u + 200 < caller.on && u < by.off && u > enroll.off {
            let utt = build_utt(&samples, u, u_end, embedder, &reference);
            by_limit = (1..utt.wins.len())
                .take_while(|&k| utt.wins[k].0 + WINDOW_MS <= caller.on)
                .last()
                .unwrap_or(0);
            merged = Some(utt);
        }
    }
    let by_only = utts
        .iter()
        .find(|(s, e)| *s > enroll.off && *e <= caller.on && *e > by.on)
        .map(|&(u, u_end)| build_utt(&samples, u, u_end, embedder, &reference));
    Ok(Some(Scene {
        id,
        paused,
        excluded,
        samples,
        by,
        caller,
        merged,
        by_only,
        by_limit,
        reference,
    }))
}

/// Wilson 95 % interval of `k / n` in percent.
fn wilson(k: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 0.0);
    }
    let (k, n, z) = (k as f64, n as f64, 1.96f64);
    let p = k / n;
    let d = 1.0 + z * z / n;
    let c = (p + z * z / (2.0 * n)) / d;
    let h = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / d;
    (100.0 * (c - h).max(0.0), 100.0 * (c + h).min(1.0))
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    percentile(v, q)
}

/// `--tap-utt`: one recorded utterance (the roundtrip's audio tap). Fits the window grid offset
/// to the logged scores when `--tap-scores` is given, then prints windows and the re-check.
fn tap(
    embedder: &Embedder,
    utt_wav: &Path,
    enroll_wav: &Path,
    logged: &[f32],
    threshold: f32,
) -> Result<String, String> {
    let samples = read_wav(utt_wav)?;
    let enroll = read_wav(enroll_wav)?;
    let (reference, _) = embedder.embed(&enroll).ok_or("enroll embed")?;
    let len_ms = samples.len() / SAMPLES_PER_MS;
    let mut best: Option<(f64, usize)> = None;
    if !logged.is_empty() {
        let mut off = PRE_ROLL_MS;
        while off + WINDOW_MS <= len_ms && off <= 2000 {
            let wins = score_windows(&samples, off, len_ms, embedder, &reference);
            let err: f64 = logged
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    wins.get(i)
                        .map_or(1.0, |w| f64::from((w.1 - l).abs()))
                })
                .sum::<f64>()
                / logged.len() as f64;
            if best.is_none_or(|b| err < b.0) {
                best = Some((err, off));
            }
            off += 20;
        }
    }
    let u = best.map_or(PRE_ROLL_MS, |b| b.1);
    let utt = build_utt(&samples, u, len_ms, embedder, &reference);
    let mut md = format!(
        "## Audio-tap reproduction\n\nUtterance `{}` ({len_ms} ms), enrollment `{}`.\n\n",
        utt_wav.file_name().unwrap_or_default().to_string_lossy(),
        enroll_wav.file_name().unwrap_or_default().to_string_lossy()
    );
    if let Some((err, off)) = best {
        md.push_str(&format!(
            "Grid fit: window 1 starts at {off} ms into the file (mean |score - logged| {err:.3}).\n\n"
        ));
    }
    md.push_str("| k | window ms | score | logged | best sub-window (start ms: score) |\n|---|---|---|---|---|\n");
    for (k, w) in utt.wins.iter().enumerate() {
        let best_sub = utt.rechecks[k].as_ref().and_then(|rc| {
            rc.subs
                .iter()
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|s| format!("{}: {:.3}", s.0, s.1))
        });
        md.push_str(&format!(
            "| {} | {}-{} | {:.3} | {} | {} |\n",
            k + 1,
            w.0,
            w.0 + WINDOW_MS,
            w.1,
            logged.get(k).map_or("-".into(), |l| format!("{l:.3}")),
            best_sub.unwrap_or_else(|| "-".into())
        ));
    }
    md.push('\n');
    for v in variants() {
        let (c, ran, cut) = run_gate(&utt, &v, threshold, usize::MAX);
        let cut_at = match c {
            Confirm::Window { k, at, sub } => {
                format!("{} ms", split_at(&samples, &utt, k, at, sub, threshold))
            }
            Confirm::None => "-".into(),
        };
        md.push_str(&format!(
            "- {}: {c:?}, cut at {cut_at} (re-checks {ran}, cut by budget {cut})\n",
            v.name()
        ));
    }
    md.push('\n');
    Ok(md)
}

fn variants() -> Vec<Variant> {
    let mut v = vec![Variant {
        band_lo: None,
        need: 1,
    }];
    for lo in BAND_LOS {
        for need in NEEDS {
            v.push(Variant {
                band_lo: Some(lo),
                need,
            });
        }
    }
    v
}

fn run() -> Result<(), String> {
    let mut fixtures: Vec<PathBuf> = Vec::new();
    let mut model = None;
    let mut threshold = 0.459f32;
    let mut out: Option<PathBuf> = None;
    let mut tap_utt = None;
    let mut tap_enroll = None;
    let mut tap_scores: Vec<f32> = Vec::new();
    let mut straddle_piper: Vec<PathBuf> = Vec::new();
    let mut librispeech: Option<PathBuf> = None;
    let mut bed: Option<PathBuf> = None;
    let mut ls_callers = 40usize;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match arg.as_str() {
            "--fixtures" => fixtures.push(PathBuf::from(value("--fixtures")?)),
            "--model" => model = Some(PathBuf::from(value("--model")?)),
            "--threshold" => {
                threshold = value("--threshold")?
                    .parse()
                    .map_err(|e| format!("--threshold: {e}"))?
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--tap-utt" => tap_utt = Some(PathBuf::from(value("--tap-utt")?)),
            "--tap-enroll" => tap_enroll = Some(PathBuf::from(value("--tap-enroll")?)),
            "--straddle-piper" => straddle_piper.push(PathBuf::from(value("--straddle-piper")?)),
            "--librispeech" => librispeech = Some(PathBuf::from(value("--librispeech")?)),
            "--bed" => bed = Some(PathBuf::from(value("--bed")?)),
            "--stt" => {
                let _ = straddle::STT_DIR.set(PathBuf::from(value("--stt")?));
            }
            "--ls-split" => {
                let _ = straddle::LS_SPLIT.set(value("--ls-split")?);
            }
            "--ls-callers" => {
                ls_callers = value("--ls-callers")?
                    .parse()
                    .map_err(|e| format!("--ls-callers: {e}"))?
            }
            "--tap-scores" => {
                tap_scores = value("--tap-scores")?
                    .split(',')
                    .map(|s| s.trim().parse::<f32>().map_err(|e| format!("--tap-scores: {e}")))
                    .collect::<Result<_, _>>()?
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let embedder = Embedder::open(&model.ok_or("--model is required")?)?;
    let mut md = format!(
        "# Near-miss re-check bench (chunk 6f, decision t)\n\nThreshold {threshold}. Sub-windows {SUB_MS} ms / hop {HOP_MS} ms over `[start(k-1), start(k) + 1500)`. Budget per window: {VERIFY_MS} ms minus the window's own embed. Excluded bystander pairs: libritts_r 120.\n\n"
    );
    if let (Some(u), Some(e)) = (&tap_utt, &tap_enroll) {
        md.push_str(&tap(&embedder, u, e, &tap_scores, threshold)?);
    }
    let mut scenes = Vec::new();
    for dir in &fixtures {
        let manifest = parse_json(
            &fs::read_to_string(dir.join("manifest.json")).map_err(|e| format!("manifest: {e}"))?,
        )?;
        let Some(Json::Arr(list)) = manifest.get("scenes") else {
            return Err("manifest has no scenes".into());
        };
        for scene in list {
            if text(scene, "kind").as_deref() != Some("other_to_caller") {
                continue;
            }
            if let Some(s) = load_scene(dir, scene, &embedder)? {
                eprintln!("{} loaded", s.id);
                scenes.push(s);
            }
        }
    }
    if !straddle_piper.is_empty() {
        md.push_str(&straddle::piper(&embedder, &straddle_piper, threshold)?);
    }
    if let Some(root) = &librispeech {
        md.push_str(&straddle::librispeech(&embedder, root, bed.as_deref(), ls_callers, threshold)?);
    }
    if !scenes.is_empty() {
        md.push_str(&tables(&scenes, threshold, &embedder));
    }
    print!("{md}");
    if let Some(path) = out {
        fs::write(&path, &md).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn tables(scenes: &[Scene], threshold: f32, embedder: &Embedder) -> String {
    let kept: Vec<&Scene> = scenes.iter().filter(|s| !s.excluded).collect();
    let nopause: Vec<&Scene> = kept.iter().copied().filter(|s| !s.paused).collect();
    let short: Vec<&Scene> = nopause
        .iter()
        .copied()
        .filter(|s| {
            let d = s.caller.off.saturating_sub(s.caller.on);
            (SHORT_MIN_MS..=SHORT_MAX_MS).contains(&d) && s.merged.is_some()
        })
        .collect();
    let merged_all: Vec<&Scene> = nopause
        .iter()
        .copied()
        .filter(|s| s.merged.is_some())
        .collect();
    let mut md = format!(
        "Scenes: {} ({} excluded pair), no pause {}, merged no-pause {}, short caller phrase ({SHORT_MIN_MS}-{SHORT_MAX_MS} ms voiced) merged {}.\n\n",
        scenes.len(),
        scenes.len() - kept.len(),
        nopause.len(),
        merged_all.len(),
        short.len()
    );
    let recovery = |set: &[&Scene], title: &str| -> String {
        let mut md = format!(
            "## Recovery: {title} ({} scenes)\n\n| variant | confirmed on caller | lost (no confirm) | confirm inside bystander (gate FA) | caller words clipped (scenes) | bystander words leaked (scenes) | re-checks run | cut by budget |\n|---|---|---|---|---|---|---|---|\n",
            set.len()
        );
        for v in variants() {
            let (mut ok, mut lost, mut fa, mut clip, mut clip_s, mut leak, mut leak_s) =
                (0, 0, 0, 0, 0, 0, 0);
            let (mut ran, mut cut) = (0, 0);
            for s in set {
                let utt = s.merged.as_ref().expect("merged");
                let (c, r, x) = run_gate(utt, &v, threshold, usize::MAX);
                ran += r;
                cut += x;
                let Confirm::Window { k, at, sub } = c else {
                    lost += 1;
                    continue;
                };
                let confirm_end = if sub { at + SUB_MS } else { at + WINDOW_MS };
                if confirm_end <= s.caller.on {
                    fa += 1;
                } else {
                    ok += 1;
                }
                let split = split_at(&s.samples, utt, k, at, sub, threshold);
                let c_cut = s.caller.words_before(split);
                let b_leak = s.by.words - s.by.words_before(split);
                if c_cut > 0 {
                    clip += c_cut;
                    clip_s += 1;
                    eprintln!("clip {} {}: split {split} caller {}-{}", v.name(), s.id, s.caller.on, s.caller.off);
                }
                if b_leak > 0 {
                    leak += b_leak;
                    leak_s += 1;
                }
            }
            md.push_str(&format!(
                "| {} | {ok} | {lost} | {fa} | {clip} ({clip_s}) | {leak} ({leak_s}) | {ran} | {cut} |\n",
                v.name()
            ));
        }
        md.push('\n');
        md
    };
    md.push_str(&recovery(&short, "short trailing caller phrase, no pause"));
    md.push_str(&recovery(&merged_all, "every merged no-pause scene"));
    // 6g decision x: the A measure (STT diff) on the normal other->caller scenes.
    let paused_all: Vec<&Scene> = kept.iter().copied().filter(|s| s.paused).collect();
    let paused_merged: Vec<&Scene> = paused_all.iter().copied().filter(|s| s.merged.is_some()).collect();
    md.push_str(&format!(
        "Paused scenes: {} (excluded pairs dropped), VAD-merged with the bystander (an other->caller split can happen): {}.\n\n",
        paused_all.len(),
        paused_merged.len()
    ));
    if straddle::STT_DIR.get().is_some() {
        for (set, title) in [
            (&merged_all, "Piper fixtures, merged no-pause"),
            (&paused_merged, "Piper fixtures, merged paused"),
        ] {
            for sc in set.iter() {
                straddle::sweep_fixture(embedder, sc, threshold);
            }
            straddle::flush(title);
        }
    }
    // False accepts on bystander-only speech.
    let by_only: Vec<&Utt> = kept.iter().filter_map(|s| s.by_only.as_ref()).collect();
    let by_part: Vec<(&Utt, usize)> = nopause
        .iter()
        .filter_map(|s| s.merged.as_ref().map(|u| (u, s.by_limit)))
        .filter(|(_, l)| *l >= 1)
        .collect();
    md.push_str("## False caller confirms on bystander-only speech\n\nRate per utterance (window 1 already `caller` excluded: the re-check does not change it). Wilson 95 % interval.\n\n| set | variant | utterances | false confirms | rate % (95 % CI) | extra vs today | extra % (95 % CI) |\n|---|---|---|---|---|---|---|\n");
    let fa_rows = |md: &mut String, name: &str, utts: &[(&Utt, usize)]| {
        let eligible: Vec<&(&Utt, usize)> = utts
            .iter()
            .filter(|(u, _)| u.wins.first().is_some_and(|w| w.1 < threshold) && u.wins.len() >= 2)
            .collect();
        let today: Vec<bool> = eligible
            .iter()
            .map(|(u, l)| matches!(run_gate(u, &variants()[0], threshold, *l).0, Confirm::Window { .. }))
            .collect();
        for v in variants() {
            let hits: Vec<bool> = eligible
                .iter()
                .map(|(u, l)| matches!(run_gate(u, &v, threshold, *l).0, Confirm::Window { .. }))
                .collect();
            let k = hits.iter().filter(|h| **h).count();
            let extra = hits.iter().zip(&today).filter(|(h, t)| **h && !**t).count();
            let n = eligible.len();
            let (a, b) = wilson(k, n);
            let (ea, eb) = wilson(extra, n);
            md.push_str(&format!(
                "| {name} | {} | {n} | {k} | {:.2} ({a:.2}-{b:.2}) | {extra} | {:.2} ({ea:.2}-{eb:.2}) |\n",
                v.name(),
                100.0 * k as f64 / n.max(1) as f64,
                100.0 * extra as f64 / n.max(1) as f64
            ));
        }
    };
    let by_only_l: Vec<(&Utt, usize)> = by_only.iter().map(|u| (*u, usize::MAX)).collect();
    fa_rows(&mut md, "bystander utterance (paused + no-pause, VAD separate)", &by_only_l);
    fa_rows(&mut md, "bystander part of merged no-pause utterances", &by_part);
    md.push('\n');
    // Cost.
    let mut win_ms: Vec<f64> = Vec::new();
    let mut rc_ms: Vec<f64> = Vec::new();
    let mut rc_embeds: Vec<f64> = Vec::new();
    for s in &kept {
        for u in s.merged.iter().chain(s.by_only.iter()) {
            for (k, w) in u.wins.iter().enumerate() {
                win_ms.push(w.2);
                if let Some(rc) = u.rechecks[k].as_ref() {
                    rc_ms.push(w.2 + rc.time_ms());
                    rc_embeds.push(rc.subs.len() as f64);
                }
            }
        }
    }
    let over = rc_ms.iter().filter(|t| **t > VERIFY_MS).count();
    md.push_str(&format!(
        "## Cost (this host)\n\n| measure | p50 | p95 | p99 | max |\n|---|---|---|---|---|\n| window embed ms | {:.1} | {:.1} | {:.1} | {:.1} |\n| window + full re-check ms | {:.1} | {:.1} | {:.1} | {:.1} |\n| sub-window embeds per re-check | {:.0} | {:.0} | {:.0} | {:.0} |\n\nFull re-checks over the {VERIFY_MS} ms budget: {over} of {} (a confirm usually stops earlier).\n\n",
        pct(&mut win_ms, 0.5), pct(&mut win_ms, 0.95), pct(&mut win_ms, 0.99), pct(&mut win_ms, 1.0),
        pct(&mut rc_ms, 0.5), pct(&mut rc_ms, 0.95), pct(&mut rc_ms, 0.99), pct(&mut rc_ms, 1.0),
        pct(&mut rc_embeds, 0.5), pct(&mut rc_embeds, 0.95), pct(&mut rc_embeds, 0.99), pct(&mut rc_embeds, 1.0),
        rc_ms.len()
    ));
    md
}

fn main() {
    if let Err(e) = run() {
        eprintln!("near_miss_bench: {e}");
        std::process::exit(1);
    }
}

/// Straddle scenes (chunk 6f, step 1): a 1.0-1.6 s caller phrase right after a bystander with no
/// pause, placed so it starts `t` (300-1200 ms) before a boundary of the gate's window grid (grid
/// origin = the VAD utterance start, as the gate computes it) and no 1.5 s window holds more than
/// 1050 ms (70 %) of it. The bystander is a tail crop ending at its last voiced frame; its length
/// is adjusted (up to 3 VAD passes) until the measured `t` is within 40 ms of the target.
mod straddle {
    use super::*;

    const LEAD_MS: usize = 800;
    const TAIL_MS: usize = 2000;
    const BY_BASE_MS: usize = 3000;
    const T_MIN: usize = 300;
    const T_MAX: usize = 1200;
    const MAX_SHARE_MS: usize = 1050;
    const MAIN_DB: f64 = -20.0;
    const BED_DB: f64 = -12.0;
    const FAR_PER_CALLER: usize = 8;
    const PIECES_PER_CALLER: usize = 5;

    struct Clip {
        audio: Vec<f32>,
        on: usize,
        off: usize,
        words: Option<usize>,
    }

    struct Placed {
        samples: Vec<f32>,
        c_on: usize,
        c_off: usize,
        words: Option<usize>,
        b_end: usize,
        u: usize,
        u_end: usize,
        reference: Vec<f32>,
        /// 6g: the caller piece alone (from its voiced onset, no bed) and the bystander crop alone,
        /// for the recognizer's word timestamps.
        caller_clean: Vec<f32>,
        by_clean: Vec<f32>,
    }

    /// 6g: `--stt <model dir>` turns on word timings from the recognizer's documented result
    /// fields (`RecognizerResult::tokens` + `timestamps`, seconds from the stream start).
    pub static STT_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    thread_local! {
        static STT: std::cell::RefCell<Option<sherpa_onnx::OnlineRecognizer>> =
            const { std::cell::RefCell::new(None) };
    }
    static DUMP_N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Word start times (ms from the start of `a`): the timestamp of each token that opens a word
    /// (BPE `\u{2581}` / leading space, or the first token). `None` without `--stt` or timestamps.
    fn word_starts(a: &[f32]) -> Option<Vec<usize>> {
        recognize_words(a).and_then(|(w, st)| (st.len() == w.len()).then_some(st))
    }

    /// Recognizer words (upper case) and their start ms (empty starts when the result has no
    /// timestamps). Same decode as the product: 100 ms chunks, finalize tail padding.
    fn recognize_words(a: &[f32]) -> Option<(Vec<String>, Vec<usize>)> {
        let dir = STT_DIR.get()?;
        STT.with(|c| {
            let mut c = c.borrow_mut();
            if c.is_none() {
                *c = Some(support::create_recognizer(dir).expect("create recognizer"));
            }
            let r = c.as_ref()?;
            let stream = r.create_stream();
            for chunk in a.chunks(1600) {
                stream.accept_waveform(16_000, chunk);
                while r.is_ready(&stream) {
                    r.decode(&stream);
                }
            }
            let tail = vec![
                0f32;
                16 * node_webrtc_rust_vendor_sherpa_onnx::FINALIZE_TAIL_PADDING_MS
            ];
            stream.accept_waveform(16_000, &tail);
            stream.input_finished();
            while r.is_ready(&stream) {
                r.decode(&stream);
            }
            let res = r.get_result(&stream)?;
            let ts = res.timestamps.unwrap_or_default();
            let (mut words, mut starts) = (Vec::<String>::new(), Vec::new());
            for (i, t) in res.tokens.iter().enumerate() {
                let new = i == 0 || t.starts_with('\u{2581}') || t.starts_with(' ');
                let body = t.trim_start_matches('\u{2581}').trim_start().to_uppercase();
                if new || words.is_empty() {
                    words.push(body);
                    if let Some(sec) = ts.get(i) {
                        starts.push((sec.max(0.0) * 1000.0) as usize);
                    }
                } else if let Some(last) = words.last_mut() {
                    last.push_str(&body);
                }
            }
            let keep: Vec<bool> = words.iter().map(|w| !w.is_empty()).collect();
            let starts = if starts.len() == words.len() {
                starts.into_iter().zip(&keep).filter(|(_, k)| **k).map(|(x, _)| x).collect()
            } else {
                Vec::new()
            };
            words.retain(|w| !w.is_empty());
            Some((words, starts))
        })
    }

    #[derive(Clone, Copy, Default)]
    struct Out {
        confirmed: bool,
        fa: bool,
        clipped_ms: usize,
        clipped_words: usize,
        leaked_ms: usize,
        ran: usize,
        cut: usize,
        /// 6g, from the recognizer's word timestamps: caller words starting before the cut,
        /// bystander words starting at or after it; cut - caller onset; first caller word start
        /// (ms after the onset, -1 without words / without `--stt`).
        words_real_clip: usize,
        words_real_leak: usize,
        delta_ms: i64,
        first_word_ms: i64,
        has_words: bool,
        /// 6g A/B per candidate cut ([`CANDS`]): A = caller reference words missing at the head of
        /// the U2 decode / bystander words inserted there; B = relative-timestamp words before the
        /// cut; cut - onset.
        has_cands: bool,
        stage2: bool,
        a_clip: [usize; NC],
        a_leak: [usize; NC],
        b_clip: [usize; NC],
        c_delta: [i64; NC],
    }

    const NC: usize = 6;
    const CANDS: [&str; NC] = [
        "ideal cut at the caller onset (STT floor)",
        "today (6b)",
        "stage 2: first caller sub-window start",
        "stage 2: forward search 250 ms",
        "stage 2: backward quiet run [p-500, p)",
        "stage 2: raw onset in [p, p+500) (6e raw_cut)",
    ];

    /// 6g decision x: `--ls-split` (default test-clean).
    pub static LS_SPLIT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    /// A normal fixture scene through [`sweep_scene`]: the caller and bystander pieces are cut
    /// from the scene (they carry its babble bed).
    pub(super) fn sweep_fixture(embedder: &Embedder, sc: &Scene, threshold: f32) {
        let Some(utt) = sc.merged.as_ref() else { return };
        let p = Placed {
            samples: sc.samples.clone(),
            c_on: sc.caller.on,
            c_off: sc.caller.off,
            words: None,
            b_end: sc.by.off,
            u: 0,
            u_end: 0,
            reference: sc.reference.clone(),
            caller_clean: ms_to(&sc.samples, sc.caller.on, sc.caller.off),
            by_clean: ms_to(&sc.samples, sc.by.on, sc.by.off),
        };
        sweep_scene(embedder, &p, utt, threshold);
    }

    pub(super) fn flush(title: &str) {
        sweep_flush(title);
    }

    // ---- 6g sweep (decision w): cut settings x near-miss variants, measured by A. ----

    const MS_BACK: [usize; 5] = [0, 100, 200, 300, 500];
    const SUBHOP: [(usize, usize); 4] = [(750, 125), (1000, 125), (1250, 125), (1250, 250)];
    const S1MIN: [usize; 4] = [300, 500, 800, usize::MAX];
    const S1S: [usize; 2] = [0, 300];
    const U2_HEAD_MS: usize = 2500;

    /// s2: 0 = forward quiet run within 500 ms (today), 1.. = first caller sub-window start
    /// minus `MS_BACK[s2 - 1]`; fb: 0 = stage 1's point (today), 1 = previous window start.
    #[derive(Clone, Copy)]
    struct Cfg {
        s2: usize,
        sh: usize,
        s1min: usize,
        s1s: usize,
        fb: usize,
        /// Decision x A'/B': stage 1 kept, its cut never later than the stage-2 cut.
        clamp: bool,
    }

    fn cfgs() -> Vec<Cfg> {
        if std::env::var("NM_CFGS").as_deref() == Ok("x") {
            let c = |s2, s1min, fb, clamp| Cfg { s2, sh: if s2 == 0 { 3 } else { 0 }, s1min, s1s: 1, fb, clamp };
            return vec![c(0, 1, 0, false), c(4, 3, 1, false), c(3, 3, 1, false), c(4, 1, 1, true), c(3, 1, 1, true)];
        }
        let mut v = Vec::new();
        for s2 in 0..=MS_BACK.len() {
            for sh in 0..SUBHOP.len() {
                for s1min in 0..S1MIN.len() {
                    for s1s in 0..S1S.len() {
                        for fb in 0..2 {
                            v.push(Cfg { s2, sh, s1min, s1s, fb, clamp: false });
                        }
                    }
                }
            }
        }
        v
    }

    fn cfg_name(c: &Cfg) -> String {
        let s2 = if c.s2 == 0 { "fwd500".to_string() } else { format!("p-{}", MS_BACK[c.s2 - 1]) };
        let s1 = if S1MIN[c.s1min] == usize::MAX { "off".to_string() } else { S1MIN[c.s1min].to_string() };
        format!(
            "s2={s2} sub={}/{} s1min={s1} s1search=+{} fb={}",
            SUBHOP[c.sh].0,
            SUBHOP[c.sh].1,
            S1S[c.s1s],
            if c.fb == 0 { "stage1" } else { "prevstart" }
        ) + if c.clamp { " s1clamp" } else { "" }
    }

    fn is_today(c: &Cfg) -> bool {
        c.s2 == 0 && c.sh == 3 && c.s1min == 1 && c.s1s == 1 && c.fb == 0 && !c.clamp
    }

    /// One scene: per variant, per config: `Some((kept words, leaked words, path, cut - onset))`
    /// when the variant confirms (path 0 stage 1, 1 stage 2, 2 fallback).
    struct SceneRec {
        floor_kept: usize,
        today_ok: bool,
        res: Vec<Vec<Option<(u16, u16, u8, i32)>>>,
        /// Today's confirm, per sub/hop: (embeds until the first caller sub-window, their ms).
        emb: [(usize, f64); 4],
    }

    static RECS: std::sync::Mutex<Vec<SceneRec>> = std::sync::Mutex::new(Vec::new());

    fn subs_for(
        samples: &[f32],
        lo: usize,
        end: usize,
        (sub, hop): (usize, usize),
        embedder: &Embedder,
        reference: &[f32],
    ) -> Vec<(usize, f32, f64)> {
        let mut out = Vec::new();
        let mut t = lo;
        while t + sub <= end {
            let (s, ms) = embedder
                .embed(slice(samples, t, t + sub))
                .map_or((0.0, 0.0), |(e, ms)| (cosine(&e, reference), ms));
            out.push((t, s, ms));
            t += hop;
        }
        out
    }

    fn sweep_cut(
        samples: &[f32],
        utt: &Utt,
        k: usize,
        nm_at: Option<usize>,
        c: &Cfg,
        subs: &[(usize, f32, f64)],
        threshold: f32,
    ) -> (usize, u8) {
        let lo = utt.wins[k - 1].0;
        let ws = utt.wins[k].0;
        let (s1, run) = quiet_run(samples, lo, ws + S1S[c.s1s]);
        if run >= S1MIN[c.s1min] && !c.clamp {
            return (s1, 0);
        }
        let stage1 = run >= S1MIN[c.s1min];
        let first = subs.iter().find(|x| x.1 >= threshold).map(|x| x.0);
        let p = match (first, nm_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let (cut, path) = match p {
            Some(p) if c.s2 == 0 => {
                let (e, r) = quiet_run(samples, p, p + FORWARD_MS);
                (if r > 0 { e } else { p }, 1)
            }
            Some(p) => (p.saturating_sub(MS_BACK[c.s2 - 1]).max(lo), 1),
            None => (if c.fb == 0 { s1 } else { lo }, 2),
        };
        if stage1 { (s1.min(cut), 0) } else { (cut, path) }
    }

    fn sweep_scene(embedder: &Embedder, p: &Placed, utt: &Utt, threshold: f32) {
        let (Some((cref, _)), Some((bref, _))) =
            (recognize_words(&p.caller_clean), recognize_words(&p.by_clean))
        else {
            return;
        };
        let total = p.samples.len() / SAMPLES_PER_MS;
        let mut cache: std::collections::HashMap<usize, (usize, usize)> = Default::default();
        let mut a_at = |cut: usize| -> (usize, usize) {
            *cache.entry(cut).or_insert_with(|| {
                let end = (cut.max(p.c_on) + U2_HEAD_MS).min(total);
                let (hyp, _) = recognize_words(slice(&p.samples, cut, end)).unwrap_or_default();
                let (del, ins) = head_align(&cref, &hyp);
                let lk = ins
                    .iter()
                    .filter(|j| bref.contains(&hyp[**j]) && !cref.contains(&hyp[**j]))
                    .count();
                (cref.len() - del.min(cref.len()), lk)
            })
        };
        let floor_kept = a_at(p.c_on).0;
        let cs = cfgs();
        let mut subs_cache: std::collections::HashMap<(usize, usize), Vec<(usize, f32, f64)>> =
            Default::default();
        let mut res = Vec::new();
        let mut today_ok = false;
        let mut emb = [(0usize, 0f64); 4];
        for (vi, v) in variants().iter().enumerate() {
            let (c, _, _) = run_gate(utt, v, threshold, usize::MAX);
            let Confirm::Window { k, at, sub } = c else {
                res.push(vec![None; cs.len()]);
                continue;
            };
            let end = if sub { at + SUB_MS } else { at + WINDOW_MS };
            if vi == 0 {
                today_ok = end > p.c_on;
            }
            let nm_at = sub.then_some(at);
            let mut row = Vec::with_capacity(cs.len());
            for c in &cs {
                let subs = subs_cache.entry((k, c.sh)).or_insert_with(|| {
                    subs_for(
                        &p.samples,
                        utt.wins[k - 1].0,
                        utt.wins[k].0 + WINDOW_MS,
                        SUBHOP[c.sh],
                        embedder,
                        &p.reference,
                    )
                });
                let (cut, path) = sweep_cut(&p.samples, utt, k, nm_at, c, subs, threshold);
                let (kept, lk) = a_at(cut);
                row.push(Some((kept as u16, lk as u16, path, cut as i32 - p.c_on as i32)));
            }
            if vi == 0 {
                for (sh, e) in emb.iter_mut().enumerate() {
                    if let Some(subs) = subs_cache.get(&(k, sh)) {
                        let n = subs
                            .iter()
                            .position(|x| x.1 >= threshold)
                            .map_or(subs.len(), |i| i + 1);
                        *e = (n, subs[..n].iter().map(|x| x.2).sum());
                    }
                }
            }
            res.push(row);
        }
        RECS.lock().unwrap().push(SceneRec { floor_kept, today_ok, res, emb });
    }

    /// Drains [`RECS`] into `NM_SWEEP_TSV` (default /tmp/sg-nm-6g-sweep.tsv) for set `title`.
    fn sweep_flush(title: &str) {
        let recs = std::mem::take(&mut *RECS.lock().unwrap());
        if recs.is_empty() {
            return;
        }
        let path = std::env::var("NM_SWEEP_TSV").unwrap_or_else(|_| "/tmp/sg-nm-6g-sweep.tsv".into());
        let mut out = String::new();
        let cs = cfgs();
        let vs = variants();
        let ti = cs.iter().position(is_today).unwrap();
        for (vi, v) in vs.iter().enumerate() {
            for (ci, c) in cs.iter().enumerate() {
                let (mut lost, mut lost_tc, mut rec_n, mut rec_kept, mut leak) = (0usize, 0usize, 0usize, 0usize, 0usize);
                let mut paths = [0usize; 3];
                let mut lost_by_path = [0usize; 3];
                let mut conf = 0usize;
                for r in &recs {
                    let x = r.res[vi][ci];
                    let kept = x.map_or(0, |x| x.0 as usize);
                    let l = r.floor_kept.saturating_sub(kept);
                    lost += l;
                    if r.today_ok {
                        lost_tc += l;
                    } else if x.is_some() {
                        rec_n += 1;
                        rec_kept += kept;
                    }
                    if let Some((_, lk, pa, _)) = x {
                        conf += 1;
                        leak += lk as usize;
                        paths[pa as usize] += 1;
                        lost_by_path[pa as usize] += l;
                    }
                }
                out.push_str(&format!(
                    "sweep\t{title}\t{vi}\t{}\t{ci}\t{}\t{}\t{}\t{conf}\t{lost}\t{lost_tc}\t{rec_n}\t{rec_kept}\t{leak}\t{}/{}/{}\t{}/{}/{}\t{}\n",
                    v.name(),
                    cfg_name(c),
                    recs.len(),
                    recs.iter().map(|r| r.floor_kept).sum::<usize>(),
                    paths[0], paths[1], paths[2],
                    lost_by_path[0], lost_by_path[1], lost_by_path[2],
                    recs.iter().filter(|r| r.today_ok).count(),
                ));
            }
        }
        // Attribution for today's cut: path x sign of cut - onset.
        let mut attr = std::collections::BTreeMap::<(u8, &str), (usize, usize)>::new();
        for r in recs.iter().filter(|r| r.today_ok) {
            if let Some((kept, _, pa, d)) = r.res[0][ti] {
                let sign = if d <= 0 { "at/before onset" } else { "after onset" };
                let e = attr.entry((pa, sign)).or_default();
                e.0 += 1;
                e.1 += r.floor_kept.saturating_sub(kept as usize);
            }
        }
        for ((pa, sign), (n, l)) in attr {
            out.push_str(&format!("attr\t{title}\t{pa}\t{sign}\t{n}\t{l}\n"));
        }
        for sh in 0..SUBHOP.len() {
            let mut n: Vec<i64> = recs.iter().filter(|r| r.today_ok).map(|r| r.emb[sh].0 as i64).collect();
            let mut ms: Vec<i64> = recs.iter().filter(|r| r.today_ok).map(|r| r.emb[sh].1.round() as i64).collect();
            out.push_str(&format!(
                "emb\t{title}\t{}/{}\t{}\t{}\t{}\t{}\n",
                SUBHOP[sh].0,
                SUBHOP[sh].1,
                pct_i(&mut n, 0.5),
                pct_i(&mut n, 0.99),
                pct_i(&mut ms, 0.5),
                pct_i(&mut ms, 0.99)
            ));
        }
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(out.as_bytes());
        }
    }

    /// 6e `raw_cut` mirror over `[lo, hi)`: a closed quiet run ends before the next loud raw frame
    /// within the smoothing reach of its smoothed end; an open run ends at its smoothed end.
    fn raw_cut(samples: &[f32], lo: usize, hi: usize) -> Option<usize> {
        let n = hi.saturating_sub(lo) / FRAME_MS;
        if n == 0 {
            return None;
        }
        let raw: Vec<f64> = (0..n).map(|i| frame_power(samples, lo + i * FRAME_MS)).collect();
        let smooth: Vec<f64> = (0..n)
            .map(|i| {
                let a = i.saturating_sub(SMOOTH_FRAMES / 2);
                let b = (i + SMOOTH_FRAMES / 2 + 1).min(n);
                db(raw[a..b].iter().sum::<f64>() / (b - a) as f64)
            })
            .collect();
        let raw_db: Vec<f64> = raw.into_iter().map(db).collect();
        let mut sorted = smooth.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let mid = (percentile(&sorted, 0.1) + percentile(&sorted, 0.9)) / 2.0;
        let (mut best_len, mut best_end, mut run) = (0usize, 0usize, 0usize);
        for (i, v) in smooth.iter().enumerate() {
            if *v < mid {
                run += 1;
                if run >= best_len {
                    best_len = run;
                    best_end = i + 1;
                }
            } else {
                run = 0;
            }
        }
        if best_len == 0 {
            return None;
        }
        if best_end >= n {
            return Some(lo + best_end * FRAME_MS);
        }
        let reach = SMOOTH_FRAMES / 2;
        let from = best_end.saturating_sub(reach).max(best_end - best_len);
        let to = (best_end + reach).min(n);
        let at = (from..to).find(|i| raw_db[*i] >= mid).unwrap_or(best_end);
        Some(lo + at * FRAME_MS)
    }

    /// Candidate cuts 1.. of [`CANDS`] for a confirm at window `k`; stage 1 (run >= 500 ms) is
    /// shared by all. Second value: the cut went to stage 2.
    fn split_cands(samples: &[f32], utt: &Utt, k: usize, threshold: f32) -> ([usize; NC - 1], bool) {
        let lo = utt.wins[k - 1].0;
        let ws = utt.wins[k].0;
        let (s1, run) = quiet_run(samples, lo, ws + SPLIT_SEARCH_MS);
        if run >= QUIET_RUN_MIN_MS {
            return ([s1; NC - 1], false);
        }
        let Some(p) = utt.rechecks[k]
            .as_ref()
            .and_then(|rc| rc.subs.iter().find(|(_, s, _)| *s >= threshold).map(|x| x.0))
        else {
            return ([s1; NC - 1], true);
        };
        let fwd = |len: usize| {
            let (e, r) = quiet_run(samples, p, p + len);
            if r > 0 { e } else { p }
        };
        let back = {
            let (e, r) = quiet_run(samples, p.saturating_sub(500), p);
            if r > 0 { e } else { p }
        };
        let raw = raw_cut(samples, p, p + FORWARD_MS).unwrap_or(p);
        ([fwd(FORWARD_MS), p, fwd(250), back, raw], true)
    }

    /// Word-level edit-distance alignment of `r` (reference) and `h` (decode). Returns the
    /// reference words deleted before the first exact match and the decode indexes inserted
    /// before it (all of both without a match).
    fn head_align(r: &[String], h: &[String]) -> (usize, Vec<usize>) {
        let (n, m) = (r.len(), h.len());
        let mut d = vec![vec![0usize; m + 1]; n + 1];
        for i in 0..=n {
            d[i][0] = i;
        }
        for j in 0..=m {
            d[0][j] = j;
        }
        for i in 1..=n {
            for j in 1..=m {
                let sub = d[i - 1][j - 1] + usize::from(r[i - 1] != h[j - 1]);
                d[i][j] = sub.min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
            }
        }
        // ops from the end: 0 match, 1 sub, 2 del (ref), 3 ins (hyp, index)
        let mut ops = Vec::new();
        let (mut i, mut j) = (n, m);
        while i > 0 || j > 0 {
            if i > 0 && j > 0 && r[i - 1] == h[j - 1] && d[i][j] == d[i - 1][j - 1] {
                ops.push((0, j - 1));
                i -= 1;
                j -= 1;
            } else if i > 0 && j > 0 && d[i][j] == d[i - 1][j - 1] + 1 {
                ops.push((1, j - 1));
                i -= 1;
                j -= 1;
            } else if i > 0 && d[i][j] == d[i - 1][j] + 1 {
                ops.push((2, 0));
                i -= 1;
            } else {
                ops.push((3, j - 1));
                j -= 1;
            }
        }
        ops.reverse();
        let (mut del, mut ins) = (0, Vec::new());
        for (op, jx) in ops {
            match op {
                0 => break,
                2 => del += 1,
                3 => ins.push(jx),
                _ => {}
            }
        }
        (del, ins)
    }

    fn ms_to(a: &[f32], s: usize, e: usize) -> Vec<f32> {
        slice(a, s, e).to_vec()
    }

    /// Voiced span (ms): first and last 20 ms frame within 30 dB of the loudest (and above -55 dBFS).
    fn voiced(a: &[f32]) -> Option<(usize, usize)> {
        let n = a.len() / SAMPLES_PER_MS / FRAME_MS;
        let dbs: Vec<f64> = (0..n).map(|i| db(frame_power(a, i * FRAME_MS))).collect();
        let max = dbs.iter().cloned().fold(f64::MIN, f64::max);
        let thr = (max - 30.0).max(-55.0);
        let first = dbs.iter().position(|d| *d > thr)?;
        let last = dbs.iter().rposition(|d| *d > thr)?;
        Some((first * FRAME_MS, (last + 1) * FRAME_MS))
    }

    /// Voiced RMS (frames within 30 dB of the loudest) to `MAIN_DB` dBFS.
    fn normalize(a: &mut [f32]) {
        let n = a.len() / SAMPLES_PER_MS / FRAME_MS;
        let p: Vec<f64> = (0..n).map(|i| frame_power(a, i * FRAME_MS)).collect();
        let max = p.iter().cloned().fold(0.0, f64::max);
        let v: Vec<f64> = p.into_iter().filter(|x| *x > max * 1e-3).collect();
        if v.is_empty() {
            return;
        }
        let rms = (v.iter().sum::<f64>() / v.len() as f64).sqrt();
        let g = (10f64.powf(MAIN_DB / 20.0) / rms) as f32;
        a.iter_mut().for_each(|x| *x *= g);
    }

    fn add_bed(s: &mut [f32], bed: Option<(&[f32], usize)>) {
        let Some((bed, off)) = bed else { return };
        if bed.is_empty() {
            return;
        }
        let rms = (bed.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / bed.len() as f64).sqrt();
        let g = (10f64.powf((MAIN_DB + BED_DB) / 20.0) / rms.max(1e-9)) as f32;
        for (i, x) in s.iter_mut().enumerate() {
            *x += bed[(off + i) % bed.len()] * g;
        }
    }

    fn pcm48(s: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(s.len() * 12);
        for x in s {
            let b = ((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes();
            for _ in 0..6 {
                out.extend_from_slice(&b);
            }
        }
        out
    }

    /// Lead + bystander tail crop of `BY_BASE_MS + extra` + caller from its voiced onset + tail.
    fn compose(
        by: &Clip,
        caller: &Clip,
        extra: usize,
        bed: Option<(&[f32], usize)>,
    ) -> Option<(Vec<f32>, usize, usize, usize)> {
        let len = BY_BASE_MS + extra;
        if by.off < by.on + len {
            return None;
        }
        let mut s = vec![0f32; LEAD_MS * SAMPLES_PER_MS];
        s.extend(ms_to(&by.audio, by.off - len, by.off));
        let c_on = LEAD_MS + len;
        s.extend(ms_to(&caller.audio, caller.on, caller.audio.len() / SAMPLES_PER_MS));
        s.extend(vec![0f32; TAIL_MS * SAMPLES_PER_MS]);
        add_bed(&mut s, bed);
        Some((s, c_on, c_on + (caller.off - caller.on), c_on))
    }

    fn place(
        by: &Clip,
        caller: &Clip,
        t: usize,
        bed: Option<(&[f32], usize)>,
        reference: &[f32],
    ) -> Result<Option<Placed>, String> {
        let mut extra = 0usize;
        for _ in 0..3 {
            let Some((samples, c_on, c_off, b_end)) = compose(by, caller, extra, bed) else {
                return Ok(None);
            };
            let utts = vad_utterances(&pcm48(&samples))?;
            let Some(&(u, u_end)) = utts.iter().find(|(_, e)| c_on < *e) else {
                return Ok(None);
            };
            if u + WINDOW_MS > c_on {
                return Ok(None);
            }
            let r = (c_on - u) % WINDOW_MS;
            let actual = WINDOW_MS - r;
            if actual.abs_diff(t) <= 40 {
                return Ok(Some(Placed {
                    samples,
                    c_on,
                    c_off,
                    words: caller.words,
                    b_end,
                    u,
                    u_end,
                    reference: reference.to_vec(),
                    caller_clean: ms_to(&caller.audio, caller.on, caller.audio.len() / SAMPLES_PER_MS),
                    by_clean: ms_to(&by.audio, by.off - (BY_BASE_MS + extra), by.off),
                }));
            }
            extra += (WINDOW_MS - t + WINDOW_MS - r) % WINDOW_MS;
        }
        Ok(None)
    }

    fn target_t(dur: usize, seed: usize) -> Option<usize> {
        let lo = T_MIN.max(dur.saturating_sub(MAX_SHARE_MS));
        let hi = T_MAX.min(MAX_SHARE_MS);
        (lo <= hi).then(|| lo + (seed.wrapping_mul(2654435761) >> 7) % (hi - lo + 1))
    }

    fn eval(embedder: &Embedder, p: &Placed, threshold: f32) -> Vec<Out> {
        let utt = build_utt(&p.samples, p.u, p.u_end, embedder, &p.reference);
        let cw = word_starts(&p.caller_clean);
        let bw = word_starts(&p.by_clean);
        let by_start = p.b_end - p.by_clean.len() / SAMPLES_PER_MS;
        sweep_scene(embedder, p, &utt, threshold);
        variants()
            .iter()
            .enumerate()
            .map(|(vi, v)| {
                let (c, ran, cut) = run_gate(&utt, v, threshold, usize::MAX);
                let mut o = Out {
                    ran,
                    cut,
                    ..Out::default()
                };
                if let Confirm::Window { k, at, sub } = c {
                    o.confirmed = true;
                    let end = if sub { at + SUB_MS } else { at + WINDOW_MS };
                    o.fa = end <= p.c_on;
                    let split = split_at(&p.samples, &utt, k, at, sub, threshold);
                    o.clipped_ms = split.saturating_sub(p.c_on);
                    o.clipped_words = p.words.map_or(0, |w| {
                        Span {
                            on: p.c_on,
                            off: p.c_off,
                            words: w,
                        }
                        .words_before(split)
                    });
                    o.leaked_ms = p.b_end.saturating_sub(split);
                    o.delta_ms = split as i64 - p.c_on as i64;
                    if let (Some(cw), Some(bw)) = (&cw, &bw) {
                        o.has_words = true;
                        o.words_real_clip = cw.iter().filter(|w| p.c_on + **w < split).count();
                        o.words_real_leak = bw.iter().filter(|w| by_start + **w >= split).count();
                        o.first_word_ms = cw.first().map_or(-1, |w| *w as i64);
                    }
                    if vi == 0 && !o.fa && std::env::var_os("NM_AB").is_some() {
                        if let (Some((cref, cts)), Some((bref, _))) =
                            (recognize_words(&p.caller_clean), recognize_words(&p.by_clean))
                        {
                            let (cands, st2) = split_cands(&p.samples, &utt, k, threshold);
                            o.has_cands = true;
                            o.stage2 = st2;
                            let mut cuts = [p.c_on; NC];
                            cuts[1..].copy_from_slice(&cands);
                            let mut seen: Vec<(usize, usize, usize)> = Vec::new();
                            for (ci, cut) in cuts.iter().enumerate() {
                                let (cl, lk) = if let Some(x) = seen.iter().find(|x| x.0 == *cut) {
                                    (x.1, x.2)
                                } else {
                                    let (hyp, _) = recognize_words(slice(&p.samples, *cut, p.samples.len() / SAMPLES_PER_MS))
                                        .unwrap_or_default();
                                    let (del, ins) = head_align(&cref, &hyp);
                                    let lk = ins
                                        .iter()
                                        .filter(|j| bref.contains(&hyp[**j]) && !cref.contains(&hyp[**j]))
                                        .count();
                                    seen.push((*cut, del, lk));
                                    (del, lk)
                                };
                                o.a_clip[ci] = cl;
                                o.a_leak[ci] = lk;
                                o.c_delta[ci] = *cut as i64 - p.c_on as i64;
                                o.b_clip[ci] = cts.first().map_or(0, |t0| {
                                    cts.iter().filter(|t| p.c_on + (**t - t0) < *cut).count()
                                });
                            }
                        }
                    }
                    if vi == 0 && k >= 1 && utt.wins[k - 1].0 >= PRE_ROLL_MS {
                        dump_split(p, &utt, k, split, threshold);
                    }
                }
                o
            })
            .collect()
    }

    /// 6g step 1: `NM_SPLIT_DUMP=<dir>` writes each today-variant confirm as `<n>.pcm` (the scene,
    /// s16le 16 kHz) plus a line in `index.txt`: `n prev_start cur_start bench_cut threshold
    /// sub-window scores...`, for the speech crate's parity test against `other_to_caller_split`.
    fn dump_split(p: &Placed, utt: &Utt, k: usize, split: usize, threshold: f32) {
        let Some(dir) = std::env::var_os("NM_SPLIT_DUMP").map(PathBuf::from) else {
            return;
        };
        let n = DUMP_N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let pcm: Vec<u8> = p
            .samples
            .iter()
            .flat_map(|x| ((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
            .collect();
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join(format!("{n}.pcm")), pcm);
        let scores: Vec<String> = utt.rechecks[k]
            .as_ref()
            .map(|rc| rc.subs.iter().map(|(_, s, _)| format!("{s}")).collect())
            .unwrap_or_default();
        let line = format!(
            "{n} {} {} {split} {threshold} {}\n",
            utt.wins[k - 1].0,
            utt.wins[k].0,
            scores.join(" ")
        );
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("index.txt")) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn pct_i(v: &mut [i64], q: f64) -> i64 {
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[((v.len() - 1) as f64 * q).round() as usize]
    }

    /// 6g step 3: today's cut (variant 0) in recognizer words.
    fn words_table(title: &str, res: &[Vec<Out>]) -> String {
        let ok: Vec<&Out> = res.iter().map(|r| &r[0]).filter(|o| o.confirmed && !o.fa).collect();
        let w: Vec<&&Out> = ok.iter().filter(|o| o.has_words).collect();
        let mut d: Vec<i64> = ok.iter().map(|o| o.delta_ms).collect();
        let mut fw: Vec<i64> = w.iter().map(|o| o.first_word_ms).filter(|x| *x >= 0).collect();
        let clip_sc = w.iter().filter(|o| o.words_real_clip > 0).count();
        let clip_tot: usize = w.iter().map(|o| o.words_real_clip).sum();
        let clip_max = w.iter().map(|o| o.words_real_clip).max().unwrap_or(0);
        let leak_sc = w.iter().filter(|o| o.words_real_leak > 0).count();
        let leak_tot: usize = w.iter().map(|o| o.words_real_leak).sum();
        let over20 = ok.iter().filter(|o| o.delta_ms > 20).count();
        let over20_nowords = w.iter().filter(|o| o.delta_ms > 20 && o.words_real_clip == 0).count();
        let d_max = d.iter().copied().max().unwrap_or(0);
        let d_min = d.iter().copied().min().unwrap_or(0);
        format!(
            "\n#### {title}: today's cut in recognizer words (6g)\n\n| confirmed on caller | with word timings | scenes clipping >=1 caller word | caller words clipped (total / max) | scenes leaking >=1 bystander word | bystander words leaked | cut - onset p10 / p50 / p90 (min..max) ms | cut > onset + 20 ms | of those, 0 words clipped | first caller word ts p50 / p90 ms |\n|---|---|---|---|---|---|---|---|---|---|\n| {} | {} | {clip_sc} | {clip_tot} / {clip_max} | {leak_sc} | {leak_tot} | {} / {} / {} ({d_min}..{d_max}) | {over20} | {over20_nowords} | {} / {} |\n",
            ok.len(),
            w.len(),
            pct_i(&mut d, 0.1),
            pct_i(&mut d, 0.5),
            pct_i(&mut d, 0.9),
            pct_i(&mut fw, 0.5),
            pct_i(&mut fw, 0.9),
        )
    }

    fn recovery_table(title: &str, res: &[Vec<Out>]) -> String {
        let mut md = recovery_table_inner(title, res);
        md.push_str(&words_table(title, res));
        md.push_str(&cands_table(title, res));
        sweep_flush(title);
        md
    }

    /// 6g A (STT diff) and B (relative timestamps) per candidate cut, today-confirmed scenes.
    fn cands_table(title: &str, res: &[Vec<Out>]) -> String {
        let w: Vec<&Out> = res.iter().map(|r| &r[0]).filter(|o| o.has_cands).collect();
        let st2 = w.iter().filter(|o| o.stage2).count();
        let mut md = format!(
            "\n#### {title}: candidate cuts, A = STT diff (U2 decode vs caller-alone decode), B = relative timestamps (6g)\n\n{} scenes ({st2} via stage 2). Clipped above floor = per scene max(0, A clipped - A clipped at the ideal onset cut). Agree = scenes where (A above floor > 0) == (B > 0).\n\n| cut | A: scenes clipped above floor | A: words above floor (total / max) | A: scenes with any head deletion | A: leaked bystander words (scenes / words) | B: scenes / words before cut | A/B agree | cut - onset p50 / p90 / max ms |\n|---|---|---|---|---|---|---|---|\n",
            w.len()
        );
        for (c, name) in CANDS.iter().enumerate() {
            let ex: Vec<usize> = w.iter().map(|o| o.a_clip[c].saturating_sub(o.a_clip[0])).collect();
            let ex_sc = ex.iter().filter(|x| **x > 0).count();
            let ex_tot: usize = ex.iter().sum();
            let ex_max = ex.iter().copied().max().unwrap_or(0);
            let any = w.iter().filter(|o| o.a_clip[c] > 0).count();
            let lk_sc = w.iter().filter(|o| o.a_leak[c] > 0).count();
            let lk: usize = w.iter().map(|o| o.a_leak[c]).sum();
            let b_sc = w.iter().filter(|o| o.b_clip[c] > 0).count();
            let b: usize = w.iter().map(|o| o.b_clip[c]).sum();
            let agree = w.iter().zip(&ex).filter(|(o, e)| (**e > 0) == (o.b_clip[c] > 0)).count();
            let mut d: Vec<i64> = w.iter().map(|o| o.c_delta[c]).collect();
            let dmax = d.iter().copied().max().unwrap_or(0);
            md.push_str(&format!(
                "| {name} | {ex_sc} | {ex_tot} / {ex_max} | {any} | {lk_sc} / {lk} | {b_sc} / {b} | {agree}/{} | {} / {} / {dmax} |\n",
                w.len(),
                pct_i(&mut d, 0.5),
                pct_i(&mut d, 0.9)
            ));
        }
        md
    }

    fn recovery_table_inner(title: &str, res: &[Vec<Out>]) -> String {
        let n = res.len();
        let today_lost: Vec<bool> = res.iter().map(|r| !r[0].confirmed || r[0].fa).collect();
        let lost_n = today_lost.iter().filter(|x| **x).count();
        let mut md = format!(
            "### {title}\n\n{n} scenes; lost today (no confirm on the caller): {lost_n}.\n\nClipped = cut later than 20 ms after the caller's voiced onset (energy onset). Leaked = bystander voiced ms after the cut.\n\n| variant | confirmed on caller | recovered of today's lost | lost | confirm inside bystander | clipped scenes (max ms) | clipped words | leaked scenes (>20 ms) / max ms | recovered: clipped (max ms) / leaked >20 ms (max ms) | re-checks | cut by budget |\n|---|---|---|---|---|---|---|---|---|---|---|\n"
        );
        for (i, v) in variants().iter().enumerate() {
            let ok = res.iter().filter(|r| r[i].confirmed && !r[i].fa).count();
            let rec = res
                .iter()
                .zip(&today_lost)
                .filter(|(r, l)| **l && r[i].confirmed && !r[i].fa)
                .count();
            let fa = res.iter().filter(|r| r[i].fa).count();
            let clipped: Vec<usize> = res
                .iter()
                .filter(|r| r[i].confirmed && r[i].clipped_ms > 20)
                .map(|r| r[i].clipped_ms)
                .collect();
            let words: usize = res.iter().map(|r| r[i].clipped_words).sum();
            let leaked: Vec<usize> = res
                .iter()
                .filter(|r| r[i].confirmed && r[i].leaked_ms > 20)
                .map(|r| r[i].leaked_ms)
                .collect();
            let recs: Vec<&Out> = res
                .iter()
                .zip(&today_lost)
                .filter(|(r, l)| **l && r[i].confirmed && !r[i].fa)
                .map(|(r, _)| &r[i])
                .collect();
            let rc: Vec<usize> = recs.iter().filter(|o| o.clipped_ms > 20).map(|o| o.clipped_ms).collect();
            let rl: Vec<usize> = recs.iter().filter(|o| o.leaked_ms > 20).map(|o| o.leaked_ms).collect();
            let rec_col = format!(
                "{} ({}) / {} ({})",
                rc.len(),
                rc.iter().max().copied().unwrap_or(0),
                rl.len(),
                rl.iter().max().copied().unwrap_or(0)
            );
            md.push_str(&format!(
                "| {} | {ok} | {rec} / {lost_n} | {} | {fa} | {} ({}) | {words} | {} / {} | {rec_col} | {} | {} |\n",
                v.name(),
                n - ok - fa,
                clipped.len(),
                clipped.iter().max().copied().unwrap_or(0),
                leaked.len(),
                leaked.iter().max().copied().unwrap_or(0),
                res.iter().map(|r| r[i].ran).sum::<usize>(),
                res.iter().map(|r| r[i].cut).sum::<usize>(),
            ));
        }
        md.push('\n');
        md
    }

    fn far_table(title: &str, utts: &[Utt], threshold: f32) -> String {
        let eligible: Vec<&Utt> = utts
            .iter()
            .filter(|u| u.wins.len() >= 2 && u.wins[0].1 < threshold)
            .collect();
        let n = eligible.len();
        let vs = variants();
        let hit = |v: &Variant, u: &Utt| {
            matches!(run_gate(u, v, threshold, usize::MAX).0, Confirm::Window { .. })
        };
        let today: Vec<bool> = eligible.iter().map(|u| hit(&vs[0], u)).collect();
        let mut md = format!(
            "### {title}\n\n{} bystander-only utterances, {n} with an `other` window 1 and 2+ windows.\n\n| variant | false confirms | rate % (95 % CI) | extra vs today | extra % (95 % CI) |\n|---|---|---|---|---|\n",
            utts.len()
        );
        for v in &vs {
            let hits: Vec<bool> = eligible.iter().map(|u| hit(v, u)).collect();
            let k = hits.iter().filter(|h| **h).count();
            let extra = hits.iter().zip(&today).filter(|(h, t)| **h && !**t).count();
            let (a, b) = wilson(k, n);
            let (ea, eb) = wilson(extra, n);
            md.push_str(&format!(
                "| {} | {k} | {:.2} ({a:.2}-{b:.2}) | {extra} | {:.2} ({ea:.2}-{eb:.2}) |\n",
                v.name(),
                100.0 * k as f64 / n.max(1) as f64,
                100.0 * extra as f64 / n.max(1) as f64
            ));
        }
        md.push('\n');
        md
    }

    pub fn piper(embedder: &Embedder, dirs: &[PathBuf], threshold: f32) -> Result<String, String> {
        // (caller clip, reference, scene id) and bystander clips.
        let mut callers: Vec<(Clip, Vec<f32>, String)> = Vec::new();
        let mut bys: Vec<(Clip, String)> = Vec::new();
        for dir in dirs {
            let manifest = parse_json(
                &fs::read_to_string(dir.join("manifest.json")).map_err(|e| format!("manifest: {e}"))?,
            )?;
            let Some(Json::Arr(list)) = manifest.get("scenes") else {
                return Err("manifest has no scenes".into());
            };
            for scene in list {
                let id = text(scene, "id").unwrap_or_default();
                let Some(Json::Arr(segs)) = scene.get("segments") else { continue };
                let role = |s: &Json| text(s, "role").unwrap_or_default();
                let is_enroll = |s: &Json| matches!(s.get("enroll"), Some(Json::Bool(true)));
                let (Some(e), Some(b), Some(c)) = (
                    segs.iter().find(|s| is_enroll(s)),
                    segs.iter().find(|s| role(s) == "bystander"),
                    segs.iter().find(|s| role(s) == "main" && !is_enroll(s)),
                ) else {
                    continue;
                };
                let (Some(e), Some(bs), Some(cs)) = (span_of(e), span_of(b), span_of(c)) else {
                    continue;
                };
                let samples = read_wav(&dir.join("scenes").join(format!("{id}.16k.wav")))?;
                let voice = text(b, "voice").unwrap_or_default();
                let sid = num(b, "sid").unwrap_or(0.0) as i64;
                if !EXCLUDED.iter().any(|(v, s)| *v == voice && *s == sid) {
                    bys.push((
                        Clip {
                            audio: ms_to(&samples, bs.on, bs.off),
                            on: 0,
                            off: bs.off - bs.on,
                            words: None,
                        },
                        id.clone(),
                    ));
                }
                let d = cs.off - cs.on;
                if (SHORT_MIN_MS..=SHORT_MAX_MS).contains(&d) {
                    let (reference, _) = embedder
                        .embed(slice(&samples, e.on, e.off))
                        .ok_or("enroll embed")?;
                    callers.push((
                        Clip {
                            audio: ms_to(&samples, cs.on, cs.off + 300),
                            on: 0,
                            off: d,
                            words: Some(cs.words),
                        },
                        reference,
                        id,
                    ));
                }
            }
        }
        let mut res = Vec::new();
        for (i, (c, reference, cid)) in callers.iter().enumerate() {
            for r in 0..3 {
                let (b, bid) = &bys[(i * 7 + r * 13 + 1) % bys.len()];
                if bid == cid {
                    continue;
                }
                let Some(t) = target_t(c.off, i * 3 + r) else { continue };
                if let Some(p) = place(b, c, t, None, reference)? {
                    res.push(eval(embedder, &p, threshold));
                }
            }
        }
        Ok(format!(
            "## Straddle scenes: Piper (fixture babble bed, {} short caller phrases, {} bystander clips)\n\nWord timings: none; caller words are placed linearly over the phrase's voiced span (energy-proportional fallback).\n\n{}",
            callers.len(),
            bys.len(),
            recovery_table("Piper straddle", &res)
        ))
    }

    fn wavs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                wavs(&p, out)?;
            } else if p.extension().is_some_and(|x| x == "wav") {
                out.push(p);
            }
        }
        Ok(())
    }

    fn load(p: &Path) -> Result<Vec<f32>, String> {
        let mut a = read_wav(p)?;
        normalize(&mut a);
        Ok(a)
    }

    pub fn librispeech(
        embedder: &Embedder,
        root: &Path,
        bed: Option<&Path>,
        max_callers: usize,
        threshold: f32,
    ) -> Result<String, String> {
        let split = LS_SPLIT.get().map_or("test-clean", |s| s.as_str());
        let test = root.join(split);
        let mut spk: Vec<PathBuf> = fs::read_dir(&test)
            .map_err(|e| format!("{}: {e}", test.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        spk.sort();
        let mut files: Vec<Vec<PathBuf>> = Vec::new();
        for s in &spk {
            let mut v = Vec::new();
            wavs(s, &mut v)?;
            files.push(v);
        }
        let bed16: Option<Vec<f32>> = match bed {
            Some(p) => {
                let bytes = fs::read(p).map_err(|e| format!("bed: {e}"))?;
                let f: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                Some(f.chunks_exact(3).map(|c| (c[0] + c[1] + c[2]) / 3.0).collect())
            }
            None => None,
        };
        let n = spk.len();
        let mut md = format!(
            "## LibriSpeech ({split}, {n} speakers, {} as callers; same-call enrollment: the first 3 s from the voiced onset of the caller's first utterance)\n\nWord timings: none (LibriSpeech ships none and no recognizer pass was run); clipping is measured in ms against the caller piece's energy onset, the piece itself is cut at its first voiced frame.\n\n",
            max_callers.min(n)
        );
        let conds: Vec<(&str, Option<&[f32]>)> = match &bed16 {
            Some(b) => vec![("Tocumen bed -12 dB", Some(b.as_slice())), ("clean", None)],
            None => vec![("clean", None)],
        };
        for (cname, cbed) in conds {
            let mut res = Vec::new();
            let mut far: Vec<Utt> = Vec::new();
            for ci in 0..max_callers.min(n) {
                let fs_c = &files[ci];
                if fs_c.len() < PIECES_PER_CALLER + 1 {
                    continue;
                }
                let e = load(&fs_c[0])?;
                let Some((eon, _)) = voiced(&e) else { continue };
                let Some((reference, _)) = embedder.embed(slice(&e, eon, eon + 3000)) else {
                    continue;
                };
                for r in 0..PIECES_PER_CALLER {
                    let a = load(&fs_c[1 + r])?;
                    let Some((on, _)) = voiced(&a) else { continue };
                    // Piece end: the quietest frame between 1000 and 1600 ms after the onset.
                    let end = (0..=30)
                        .map(|i| on + SHORT_MIN_MS + i * FRAME_MS)
                        .min_by(|x, y| frame_power(&a, *x).total_cmp(&frame_power(&a, *y)))
                        .unwrap_or(on + SHORT_MIN_MS);
                    let caller = Clip {
                        audio: ms_to(&a, on, end),
                        on: 0,
                        off: end - on,
                        words: None,
                    };
                    let bs = (ci + 1 + r * 3) % n;
                    let bf = &files[bs];
                    let ba = load(&bf[(ci + r) % bf.len()])?;
                    let Some((bon, boff)) = voiced(&ba) else { continue };
                    let by = Clip {
                        audio: ba,
                        on: bon,
                        off: boff,
                        words: None,
                    };
                    let Some(t) = target_t(caller.off, ci * PIECES_PER_CALLER + r) else {
                        continue;
                    };
                    let bedo = cbed.map(|b| (b, (ci * 7919 + r * 104729) % b.len()));
                    if let Some(p) = place(&by, &caller, t, bedo, &reference)? {
                        res.push(eval(embedder, &p, threshold));
                    }
                }
                for r in 0..FAR_PER_CALLER {
                    let bs = (ci + 1 + r) % n;
                    let bf = &files[bs];
                    let ba = load(&bf[(ci + r + 7) % bf.len()])?;
                    let mut s = vec![0f32; LEAD_MS * SAMPLES_PER_MS];
                    s.extend_from_slice(slice(&ba, 0, 15_000));
                    s.extend(vec![0f32; TAIL_MS * SAMPLES_PER_MS]);
                    add_bed(&mut s, cbed.map(|b| (b, (ci * 31 + r * 7907) % b.len())));
                    for (u, ue) in vad_utterances(&pcm48(&s))? {
                        if ue >= u + 3000 {
                            far.push(build_utt(&s, u, ue, embedder, &reference));
                        }
                    }
                }
                eprintln!("librispeech {cname} caller {ci} done");
            }
            md.push_str(&recovery_table(&format!("LibriSpeech straddle, {cname}"), &res));
            md.push_str(&far_table(&format!("LibriSpeech bystander-only, {cname}"), &far, threshold));
        }
        Ok(md)
    }
}
