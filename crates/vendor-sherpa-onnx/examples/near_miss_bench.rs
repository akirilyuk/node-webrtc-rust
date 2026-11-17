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
//! The description below is the chunk 6f bench as first run (1250/250 sub-windows, stage 1 quiet
//! run). The product shipped in chunk 6g differs: stage 1 is removed, the sub-windows are
//! `SPLIT_STAGE2_SUB_MS`/`SPLIT_STAGE2_HOP_MS` (750/125 ms) from the previous window start, the
//! cut is the first `caller` sub-window start minus `SPLIT_BACKOFF_MS` (config A), and the
//! near-miss band is `[NEAR_MISS_MIN_SCORE, threshold)` with one sub-window. Config A in
//! `NEAR_MISS_BENCH.md` (Configs) is the shipped rule.
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
//!   budget (500 ms minus the window's own embed, as `scan_caller_sub_window` bounds it). A re-check that
//!   would run past the budget is cut off there and confirms nothing (counted).
//! - **Second cut** (`--second-cut`, chunk 7h, decision jj): on each split-born U2 the caller ->
//!   other watch is simulated on the product's U2 window grid from its caller floor, rules today /
//!   jj / option 2; optional `--takeover-db <dB>` appends a bystander takeover after the caller.
//!   Table: caller words cut out of U2, bystander-2 words left, takeovers split later or not split.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

#[path = "support/cand.rs"]
mod cand;
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
            .embed(slice(
                samples,
                start.saturating_sub(PRE_ROLL_MS),
                start + WINDOW_MS,
            ))
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

fn load_scene(fixtures: &Path, scene: &Json, embedder: &Embedder) -> Result<Option<Scene>, String> {
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

/// `--window-dist`: per fixture set, the distribution of the 1.5 s TitaNet window scores of the
/// merged and bystander-only utterances (the windows the rules act on): p10 / p50 / p90, the
/// share below the confident-other bound and the share in the near-miss band.
fn window_dist_table(dist: &mut std::collections::BTreeMap<String, Vec<f64>>, thr: f32) -> String {
    let mut md = format!(
        "## Window score distribution (threshold {thr})\n\n| set | windows | p10 | p50 | p90 | share < 0.20 | share in [0.30, thr) |\n| --- | --- | --- | --- | --- | --- | --- |\n"
    );
    for (dir, v) in dist.iter_mut() {
        let n = v.len();
        let low = v.iter().filter(|x| **x < 0.20).count();
        let nm = v
            .iter()
            .filter(|x| **x >= 0.30 && **x < f64::from(thr))
            .count();
        let (p10, p50, p90) = (pct(v, 0.10), pct(v, 0.50), pct(v, 0.90));
        md.push_str(&format!(
            "| {dir} | {n} | {p10:.3} | {p50:.3} | {p90:.3} | {:.1} % | {:.1} % |\n",
            100.0 * low as f64 / n.max(1) as f64,
            100.0 * nm as f64 / n.max(1) as f64
        ));
    }
    md
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
                .map(|(i, l)| wins.get(i).map_or(1.0, |w| f64::from((w.1 - l).abs())))
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
    let mut window_dist = false;
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
            "--evidence" => {
                let _ = straddle::EVIDENCE.set(());
                let c = cand::cand();
                if c.c2 || c.enroll_ms != cand::ENROLL_MS_DEFAULT {
                    eprintln!("note: --evidence ignores NM_CAND c2 / e<ms>: the Piper fixtures enroll from one sentence");
                }
                if c.c7_nm || c.c7_scan || c.c7_run != 2 {
                    eprintln!("note: --evidence ignores NM_CAND c7*: this path has no caller -> other watch");
                }
            }
            "--utterance-loss" => {
                let _ = straddle::UTT_LOSS.set(());
            }
            "--second-cut" => {
                let _ = straddle::EVIDENCE.set(());
                let _ = straddle::SECOND_CUT.set(());
            }
            "--takeover-db" => {
                let db: f64 = value("--takeover-db")?
                    .parse()
                    .map_err(|e| format!("--takeover-db: {e}"))?;
                let _ = straddle::TAKEOVER_DB.set(db);
            }
            "--ls-split" => {
                let _ = straddle::LS_SPLIT.set(value("--ls-split")?);
            }
            "--ls-callers" => {
                ls_callers = value("--ls-callers")?
                    .parse()
                    .map_err(|e| format!("--ls-callers: {e}"))?
            }
            "--window-dist" => window_dist = true,
            "--tap-scores" => {
                tap_scores = value("--tap-scores")?
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<f32>()
                            .map_err(|e| format!("--tap-scores: {e}"))
                    })
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
    let mut dist: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
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
                if window_dist {
                    let v = dist.entry(dir.display().to_string()).or_default();
                    for u in [&s.merged, &s.by_only].into_iter().flatten() {
                        v.extend(u.wins.iter().map(|w| f64::from(w.1)));
                    }
                }
                scenes.push(s);
            }
        }
    }
    if window_dist {
        md.push_str(&window_dist_table(&mut dist, threshold));
        print!("{md}");
        if let Some(path) = out {
            fs::write(&path, &md).map_err(|e| format!("write {}: {e}", path.display()))?;
        }
        return Ok(());
    }
    if straddle::UTT_LOSS.get().is_some() {
        let root = librispeech
            .as_ref()
            .ok_or("--utterance-loss needs --librispeech")?;
        straddle::utterance_loss(&embedder, root, bed.as_deref(), ls_callers, threshold)?;
        md.push_str(&straddle::utterance_loss_tables());
        print!("{md}");
        if let Some(path) = out {
            fs::write(&path, &md).map_err(|e| format!("write {}: {e}", path.display()))?;
        }
        return Ok(());
    }
    if !straddle_piper.is_empty() {
        md.push_str(&straddle::piper(&embedder, &straddle_piper, threshold)?);
    }
    if let Some(root) = &librispeech {
        md.push_str(&straddle::librispeech(
            &embedder,
            root,
            bed.as_deref(),
            ls_callers,
            threshold,
        )?);
    }
    if !scenes.is_empty() {
        md.push_str(&tables(&scenes, threshold, &embedder));
    }
    if straddle::SECOND_CUT.get().is_some() {
        md.push_str(&straddle::second_cut_table());
    } else if straddle::EVIDENCE.get().is_some() {
        straddle::caller_only_piper(&embedder, &straddle_piper, bed.as_deref(), threshold)?;
        if let Some(root) = &librispeech {
            straddle::caller_only_ls(&embedder, root, bed.as_deref(), ls_callers, threshold)?;
        }
        md.push_str(&straddle::evidence_tables());
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
                    eprintln!(
                        "clip {} {}: split {split} caller {}-{}",
                        v.name(),
                        s.id,
                        s.caller.on,
                        s.caller.off
                    );
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
    let paused_merged: Vec<&Scene> = paused_all
        .iter()
        .copied()
        .filter(|s| s.merged.is_some())
        .collect();
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
            .map(|(u, l)| {
                matches!(
                    run_gate(u, &variants()[0], threshold, *l).0,
                    Confirm::Window { .. }
                )
            })
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
    fa_rows(
        &mut md,
        "bystander utterance (paused + no-pause, VAD separate)",
        &by_only_l,
    );
    fa_rows(
        &mut md,
        "bystander part of merged no-pause utterances",
        &by_part,
    );
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
    use node_webrtc_rust_speech::speaker_gate::NEAR_MISS_MIN_SCORE;

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
        /// 7h `--takeover-db`: the second bystander's onset (right after the caller's voiced end,
        /// no pause) and its clean audio.
        by2: Option<(usize, Vec<f32>)>,
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
            let tail =
                vec![0f32; 16 * node_webrtc_rust_vendor_sherpa_onnx::FINALIZE_TAIL_PADDING_MS];
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
                starts
                    .into_iter()
                    .zip(&keep)
                    .filter(|(_, k)| **k)
                    .map(|(x, _)| x)
                    .collect()
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
        let Some(utt) = sc.merged.as_ref() else {
            return;
        };
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
            by2: None,
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
            let c = |s2, s1min, fb, clamp| Cfg {
                s2,
                sh: if s2 == 0 { 3 } else { 0 },
                s1min,
                s1s: 1,
                fb,
                clamp,
            };
            return vec![
                c(0, 1, 0, false),
                c(4, 3, 1, false),
                c(3, 3, 1, false),
                c(4, 1, 1, true),
                c(3, 1, 1, true),
            ];
        }
        let mut v = Vec::new();
        for s2 in 0..=MS_BACK.len() {
            for sh in 0..SUBHOP.len() {
                for s1min in 0..S1MIN.len() {
                    for s1s in 0..S1S.len() {
                        for fb in 0..2 {
                            v.push(Cfg {
                                s2,
                                sh,
                                s1min,
                                s1s,
                                fb,
                                clamp: false,
                            });
                        }
                    }
                }
            }
        }
        v
    }

    fn cfg_name(c: &Cfg) -> String {
        let s2 = if c.s2 == 0 {
            "fwd500".to_string()
        } else {
            format!("p-{}", MS_BACK[c.s2 - 1])
        };
        let s1 = if S1MIN[c.s1min] == usize::MAX {
            "off".to_string()
        } else {
            S1MIN[c.s1min].to_string()
        };
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
    /// Per config: (lost, leaked, flags, cut offset) of one scene; `None` when it did not confirm.
    type CfgRes = Option<(u16, u16, u8, i32)>;
    /// Sub-window scores per (sub, hop): (start, score, embed ms).
    type SubsCache = std::collections::HashMap<(usize, usize), Vec<(usize, f32, f64)>>;

    struct SceneRec {
        floor_kept: usize,
        today_ok: bool,
        res: Vec<Vec<CfgRes>>,
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
        if stage1 {
            (s1.min(cut), 0)
        } else {
            (cut, path)
        }
    }

    fn sweep_scene(embedder: &Embedder, p: &Placed, utt: &Utt, threshold: f32) {
        let (Some((cref, _)), Some((bref, _))) = (
            recognize_words(&p.caller_clean),
            recognize_words(&p.by_clean),
        ) else {
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
        if EVIDENCE.get().is_some() {
            // 6j: the product gate's other -> caller confirm and cut (config A), and U1's scores;
            // 7f: the same per veto mode.
            let mut cache = SubCache::default();
            let modes: Vec<ModeRec> = (0..O2C_MODES.len())
                .map(|m| {
                    let (o, st, end_caller) = product_o2c(
                        &p.samples,
                        utt,
                        embedder,
                        &p.reference,
                        threshold,
                        m,
                        &mut cache,
                    );
                    match o {
                        Some((k, cut, u1, _)) => {
                            let (kept, lk) = a_at(cut);
                            ModeRec {
                                k: Some(k),
                                split: Some((floor_kept.saturating_sub(kept), lk)),
                                u1,
                                nosplit_lost: 0,
                                early: cut + 1500 < p.c_on,
                                st,
                                end_caller,
                                c4acc: false,
                            }
                        }
                        None => {
                            let (c4acc, lost) = no_confirm(
                                &utt.wins,
                                threshold,
                                cand().c4,
                                end_caller,
                                floor_kept,
                                false,
                            );
                            ModeRec {
                                k: None,
                                split: None,
                                u1: Vec::new(),
                                nosplit_lost: lost,
                                early: false,
                                st,
                                end_caller,
                                c4acc,
                            }
                        }
                    }
                })
                .collect();
            if SECOND_CUT.get().is_some() {
                second_cut_scene(p, utt, embedder, threshold, &cref, &mut cache);
            }
            let m0 = modes[0].clone();
            let rec = EvRec {
                set: String::new(),
                caller_only: false,
                u1: m0.u1,
                split: m0.split,
                upgrade_leak: if m0.split.is_some() || m0.c4acc {
                    bref.len()
                } else {
                    0
                },
                nosplit_lost: m0.nosplit_lost,
                ref_words: cref.len(),
                floor_gap: cref.len().saturating_sub(floor_kept),
                modes,
            };
            EV_PENDING.lock().unwrap().push(rec);
            return;
        }
        let cs = cfgs();
        let mut subs_cache: SubsCache = Default::default();
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
                row.push(Some((
                    kept as u16,
                    lk as u16,
                    path,
                    cut as i32 - p.c_on as i32,
                )));
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
        RECS.lock().unwrap().push(SceneRec {
            floor_kept,
            today_ok,
            res,
            emb,
        });
    }

    /// Drains [`RECS`] into `NM_SWEEP_TSV` (default /tmp/sg-nm-6g-sweep.tsv) for set `title`.
    fn sweep_flush(title: &str) {
        {
            let mut pending = std::mem::take(&mut *EV_PENDING.lock().unwrap());
            for r in &mut pending {
                r.set = title.to_string();
            }
            EV.lock().unwrap().extend(pending);
            let mut c2 = std::mem::take(&mut *C2_PENDING.lock().unwrap());
            for r in &mut c2 {
                r.set = title.to_string();
            }
            C2.lock().unwrap().extend(c2);
        }
        let recs = std::mem::take(&mut *RECS.lock().unwrap());
        if recs.is_empty() {
            return;
        }
        let path =
            std::env::var("NM_SWEEP_TSV").unwrap_or_else(|_| "/tmp/sg-nm-6g-sweep.tsv".into());
        let mut out = String::new();
        let cs = cfgs();
        let vs = variants();
        let ti = cs.iter().position(is_today).unwrap();
        for (vi, v) in vs.iter().enumerate() {
            for (ci, c) in cs.iter().enumerate() {
                let (mut lost, mut lost_tc, mut rec_n, mut rec_kept, mut leak) =
                    (0usize, 0usize, 0usize, 0usize, 0usize);
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
                let sign = if d <= 0 {
                    "at/before onset"
                } else {
                    "after onset"
                };
                let e = attr.entry((pa, sign)).or_default();
                e.0 += 1;
                e.1 += r.floor_kept.saturating_sub(kept as usize);
            }
        }
        for ((pa, sign), (n, l)) in attr {
            out.push_str(&format!("attr\t{title}\t{pa}\t{sign}\t{n}\t{l}\n"));
        }
        for (sh, sub_hop) in SUBHOP.iter().enumerate() {
            let mut n: Vec<i64> = recs
                .iter()
                .filter(|r| r.today_ok)
                .map(|r| r.emb[sh].0 as i64)
                .collect();
            let mut ms: Vec<i64> = recs
                .iter()
                .filter(|r| r.today_ok)
                .map(|r| r.emb[sh].1.round() as i64)
                .collect();
            out.push_str(&format!(
                "emb\t{title}\t{}/{}\t{}\t{}\t{}\t{}\n",
                sub_hop.0,
                sub_hop.1,
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
    fn split_cands(
        samples: &[f32],
        utt: &Utt,
        k: usize,
        threshold: f32,
    ) -> ([usize; NC - 1], bool) {
        let lo = utt.wins[k - 1].0;
        let ws = utt.wins[k].0;
        let (s1, run) = quiet_run(samples, lo, ws + SPLIT_SEARCH_MS);
        if run >= QUIET_RUN_MIN_MS {
            return ([s1; NC - 1], false);
        }
        let Some(p) = utt.rechecks[k].as_ref().and_then(|rc| {
            rc.subs
                .iter()
                .find(|(_, s, _)| *s >= threshold)
                .map(|x| x.0)
        }) else {
            return ([s1; NC - 1], true);
        };
        let fwd = |len: usize| {
            let (e, r) = quiet_run(samples, p, p + len);
            if r > 0 {
                e
            } else {
                p
            }
        };
        let back = {
            let (e, r) = quiet_run(samples, p.saturating_sub(500), p);
            if r > 0 {
                e
            } else {
                p
            }
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
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in d[0].iter_mut().enumerate() {
            *cell = j;
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
        let rms =
            (bed.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / bed.len() as f64).sqrt();
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
        match TAKEOVER_DB.get() {
            // 7h: the caller's voiced piece, then a second bystander with no pause.
            Some(db) => {
                s.extend(ms_to(&caller.audio, caller.on, caller.off));
                s.extend(takeover_clip(by, *db));
            }
            None => s.extend(ms_to(
                &caller.audio,
                caller.on,
                caller.audio.len() / SAMPLES_PER_MS,
            )),
        }
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
                    caller_clean: ms_to(
                        &caller.audio,
                        caller.on,
                        caller.audio.len() / SAMPLES_PER_MS,
                    ),
                    by_clean: ms_to(&by.audio, by.off - (BY_BASE_MS + extra), by.off),
                    by2: TAKEOVER_DB.get().map(|db| (c_off, takeover_clip(by, *db))),
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
                        if let (Some((cref, cts)), Some((bref, _))) = (
                            recognize_words(&p.caller_clean),
                            recognize_words(&p.by_clean),
                        ) {
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
                                    let (hyp, _) = recognize_words(slice(
                                        &p.samples,
                                        *cut,
                                        p.samples.len() / SAMPLES_PER_MS,
                                    ))
                                    .unwrap_or_default();
                                    let (del, ins) = head_align(&cref, &hyp);
                                    let lk = ins
                                        .iter()
                                        .filter(|j| {
                                            bref.contains(&hyp[**j]) && !cref.contains(&hyp[**j])
                                        })
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
                        dump_split(embedder, p, &utt, k, threshold);
                    }
                }
                o
            })
            .collect()
    }

    /// 6g: `NM_SPLIT_DUMP=<dir>` writes each confirm under today's gate as `<n>.pcm` (the scene,
    /// s16le 16 kHz) plus a line in `index.txt`: `n prev_start cur_start bench_cut threshold
    /// sub-window scores...`, for the speech crate's `bench_split_parity` test. The cut and the
    /// scores are config A's (750/125 sub-windows from the previous window's start, first
    /// `caller` one minus 300 ms, clamped; the previous window's start without one), the product's
    /// `other` -> `caller` cut since chunk 6g.
    fn dump_split(embedder: &Embedder, p: &Placed, utt: &Utt, k: usize, threshold: f32) {
        dump_split_raw(embedder, &p.samples, &p.reference, utt, k, threshold);
    }

    fn dump_split_raw(
        embedder: &Embedder,
        samples: &[f32],
        reference: &[f32],
        utt: &Utt,
        k: usize,
        threshold: f32,
    ) {
        let Some(dir) = std::env::var_os("NM_SPLIT_DUMP").map(PathBuf::from) else {
            return;
        };
        let n = DUMP_N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|x| ((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
            .collect();
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(dir.join(format!("{n}.pcm")), pcm);
        let lo = utt.wins[k - 1].0;
        let subs = subs_for(
            samples,
            lo,
            utt.wins[k].0 + WINDOW_MS,
            (750, 125),
            embedder,
            reference,
        );
        let split = subs
            .iter()
            .find(|x| x.1 >= threshold)
            .map_or(lo, |x| x.0.saturating_sub(300).max(lo));
        let scores: Vec<String> = subs.iter().map(|(_, s, _)| format!("{s}")).collect();
        let line = format!(
            "{n} {} {} {split} {threshold} {}\n",
            utt.wins[k - 1].0,
            utt.wins[k].0,
            scores.join(" ")
        );
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("index.txt"))
        {
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
        let ok: Vec<&Out> = res
            .iter()
            .map(|r| &r[0])
            .filter(|o| o.confirmed && !o.fa)
            .collect();
        let w: Vec<&&Out> = ok.iter().filter(|o| o.has_words).collect();
        let mut d: Vec<i64> = ok.iter().map(|o| o.delta_ms).collect();
        let mut fw: Vec<i64> = w
            .iter()
            .map(|o| o.first_word_ms)
            .filter(|x| *x >= 0)
            .collect();
        let clip_sc = w.iter().filter(|o| o.words_real_clip > 0).count();
        let clip_tot: usize = w.iter().map(|o| o.words_real_clip).sum();
        let clip_max = w.iter().map(|o| o.words_real_clip).max().unwrap_or(0);
        let leak_sc = w.iter().filter(|o| o.words_real_leak > 0).count();
        let leak_tot: usize = w.iter().map(|o| o.words_real_leak).sum();
        let over20 = ok.iter().filter(|o| o.delta_ms > 20).count();
        let over20_nowords = w
            .iter()
            .filter(|o| o.delta_ms > 20 && o.words_real_clip == 0)
            .count();
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
            let ex: Vec<usize> = w
                .iter()
                .map(|o| o.a_clip[c].saturating_sub(o.a_clip[0]))
                .collect();
            let ex_sc = ex.iter().filter(|x| **x > 0).count();
            let ex_tot: usize = ex.iter().sum();
            let ex_max = ex.iter().copied().max().unwrap_or(0);
            let any = w.iter().filter(|o| o.a_clip[c] > 0).count();
            let lk_sc = w.iter().filter(|o| o.a_leak[c] > 0).count();
            let lk: usize = w.iter().map(|o| o.a_leak[c]).sum();
            let b_sc = w.iter().filter(|o| o.b_clip[c] > 0).count();
            let b: usize = w.iter().map(|o| o.b_clip[c]).sum();
            let agree = w
                .iter()
                .zip(&ex)
                .filter(|(o, e)| (**e > 0) == (o.b_clip[c] > 0))
                .count();
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
            let rc: Vec<usize> = recs
                .iter()
                .filter(|o| o.clipped_ms > 20)
                .map(|o| o.clipped_ms)
                .collect();
            let rl: Vec<usize> = recs
                .iter()
                .filter(|o| o.leaked_ms > 20)
                .map(|o| o.leaked_ms)
                .collect();
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
            matches!(
                run_gate(u, v, threshold, usize::MAX).0,
                Confirm::Window { .. }
            )
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

    // ---- 6j (decision aa): an other -> caller split needs confident other evidence in U1. ----

    /// 6j: `--evidence` replaces the 6g cut sweep with the U1-evidence bench (`EVIDENCE.md`
    /// section of `NEAR_MISS_BENCH.md`).
    pub static EVIDENCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    /// Product constants: `NEAR_MISS_MIN_SCORE`, `SPLIT_STAGE2_SUB_MS` / `_HOP_MS`, `SPLIT_BACKOFF_MS`.
    const P_NM_LO: f32 = 0.30;
    const P_SUB: usize = 750;
    const P_HOP: usize = 125;
    const P_BACK: usize = 300;
    /// Caller-only utterances per LibriSpeech caller (files not used by the straddle pieces).
    const CO_PIECES: usize = 6;
    /// Gap between concatenated Piper caller phrases (merged by the VAD into one utterance).
    const CO_GAP_MS: usize = 150;

    /// One utterance of the 6j bench. `split`: the product gate confirms an other -> caller change
    /// (lost caller words above the floor, leaked bystander words, both measure A at the cut).
    /// Without a split the rule never applies: `nosplit_lost` words are lost under every rule.
    /// A rule that refuses the split upgrades U1 to caller: no caller word is lost and
    /// `upgrade_leak` bystander words reach the caller final.
    struct EvRec {
        set: String,
        caller_only: bool,
        u1: Vec<f32>,
        split: Option<(usize, usize)>,
        upgrade_leak: usize,
        nosplit_lost: usize,
        /// Reference words, and the STT floor: reference words measure A misses with the cut at
        /// the caller onset (bystander->caller) or at U1's start (caller-only), no gate.
        ref_words: usize,
        floor_gap: usize,
        /// 7f: one entry per [`O2C_MODES`] (index 0 = today, the fields above).
        modes: Vec<ModeRec>,
    }

    /// 7f: LibriSpeech bystander-only utterances per set: eligible (window 1 `other`, 2+
    /// windows), product other -> caller confirms per mode, utterances that end with the deferred
    /// veto pending (all `caller`), and bystander words labelled `caller` (today: after the cut
    /// of a split; deferred veto: after its cut, or the whole utterance when it ends pending).
    #[derive(Default)]
    struct FarO2c {
        set: String,
        eligible: usize,
        hits: [usize; 4],
        end_caller: usize,
        caller_words: [usize; 2],
    }

    static FAR_O2C: std::sync::Mutex<Vec<FarO2c>> = std::sync::Mutex::new(Vec::new());

    fn far_o2c(
        set: &str,
        samples: &[f32],
        utt: &Utt,
        (u, ue): (usize, usize),
        embedder: &Embedder,
        reference: &[f32],
        threshold: f32,
    ) {
        if utt.wins.len() < 2 || utt.wins[0].1 >= threshold {
            return;
        }
        let words =
            |from: usize| recognize_words(slice(samples, from, ue)).map_or(0, |(w, _)| w.len());
        let mut cache = SubCache::default();
        let mut hit = [0usize; 4];
        let (mut end_caller, mut cw) = (0, [0usize; 2]);
        for (m, h) in hit.iter_mut().enumerate() {
            let (o, _, end) =
                product_o2c(samples, utt, embedder, reference, threshold, m, &mut cache);
            *h = usize::from(o.is_some());
            let cut = o.map(|(_, c, _, _)| c).or(end.then_some(u));
            if m == 3 {
                end_caller = usize::from(end);
            }
            let slot = match m {
                0 => Some(0),
                3 => Some(1),
                _ => None,
            };
            if let (Some(c), Some(i)) = (cut, slot) {
                cw[i] = words(c);
            }
        }
        let set = format!("LS bystander-only {set}");
        let mut far = FAR_O2C.lock().unwrap();
        let i = match far.iter().position(|e| e.set == set) {
            Some(i) => i,
            None => {
                far.push(FarO2c {
                    set,
                    ..Default::default()
                });
                far.len() - 1
            }
        };
        let e = &mut far[i];
        e.eligible += 1;
        for (acc, h) in e.hits.iter_mut().zip(hit) {
            *acc += h;
        }
        e.end_caller += end_caller;
        e.caller_words[0] += cw[0];
        e.caller_words[1] += cw[1];
    }

    /// 7f table (decision gg): today vs the veto modes under the shipped split rule (decision bb,
    /// a U1 window below 0.20 splits, else the change upgrades). Lost = split lost above the floor
    /// plus words of utterances that no longer confirm; leaked = split leak, or every bystander
    /// word on an upgrade. Confirms under a veto mode are a subset of today's (a veto only skips a
    /// window), so today's `upgrade_leak` applies.
    fn veto_table(recs: &[EvRec], sets: &[(String, bool)]) -> String {
        let bb = |u: &[f32]| u.iter().any(|x| *x < 0.20);
        let mut md = String::from(
            "## Scan-miss veto for other -> caller (chunk 7f, decision gg)\n\nShipped split rule (decision bb). Per set and mode: lost caller words above the floor (measure A: split loss + no-confirm loss) / leaked bystander words; confirms at today's window / at a later window (delayed by 1 / by 2+) / no longer confirmed; rule outcome changed (split -> upgrade / upgrade -> split); caller-only false splits; confirms more than 1500 ms before the caller onset (bystander-only window, the n02 shape); scans / sub-window embeds / total ms / longest scan ms.\n\n| set | mode | lost (A) | leaked | same window | delayed 1 / 2+ | no confirm | split->upgrade / upgrade->split | caller-only false splits | early confirms | end of speech with the veto pending -> whole utterance `caller` (late barge) | scans / embeds / ms / max ms |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
        );
        for (set, co) in sets {
            let rs: Vec<&EvRec> = recs.iter().filter(|r| r.set == *set).collect();
            for (m, name) in O2C_MODES.iter().enumerate() {
                let (mut lost, mut leak, mut same, mut d1, mut d2, mut gone) = (0, 0, 0, 0, 0, 0);
                let (mut s2u, mut u2s, mut cofs, mut early, mut endc) = (0, 0, 0, 0, 0);
                let mut c4n = 0;
                let (mut scans, mut embeds, mut ms, mut max_ms) = (0, 0, 0.0, 0f64);
                for r in &rs {
                    let (t, x) = (&r.modes[0], &r.modes[m]);
                    lost += x.nosplit_lost;
                    if x.end_caller {
                        endc += 1;
                        leak += r.upgrade_leak;
                    }
                    if x.c4acc {
                        c4n += 1;
                        leak += r.upgrade_leak;
                    }
                    if let Some((l, k)) = x.split {
                        if bb(&x.u1) {
                            lost += l;
                            leak += k;
                            cofs += usize::from(*co);
                        } else {
                            leak += r.upgrade_leak;
                        }
                        early += usize::from(x.early);
                    }
                    match (t.k, x.k) {
                        (Some(a), Some(b)) if a == b => same += 1,
                        (Some(a), Some(b)) if b == a + 1 => d1 += 1,
                        (Some(_), Some(_)) => d2 += 1,
                        (Some(_), None) if !x.end_caller => gone += 1,
                        _ => {}
                    }
                    if t.split.is_some() && x.split.is_some() {
                        match (bb(&t.u1), bb(&x.u1)) {
                            (true, false) => s2u += 1,
                            (false, true) => u2s += 1,
                            _ => {}
                        }
                    }
                    scans += x.st.scans;
                    embeds += x.st.embeds;
                    ms += x.st.ms;
                    max_ms = max_ms.max(x.st.max_ms);
                }
                md.push_str(&format!(
                    "| {set} | {name} | {lost} | {leak} | {same} | {d1} / {d2} | {gone} | {s2u} / {u2s} | {} | {early} | {endc} | {scans} / {embeds} / {ms:.0} / {max_ms:.0} |\n",
                    if *co { cofs.to_string() } else { "-".into() }
                ));
                if cand().c4 > 0 {
                    md.push_str(&format!(
                        "|  | {name} C4 accepts (no confirm, whole utterance `caller`) | {c4n} |  |  |  |  |  |  |  |  |  |\n"
                    ));
                }
            }
        }
        let far = FAR_O2C.lock().unwrap();
        if !far.is_empty() {
            md.push_str("\n### Bystander-only false splits (LibriSpeech bystander-only utterances, window 1 `other`, 2+ windows)\n\nConfirms per mode; deferred veto: utterances that end with the veto pending (whole utterance `caller`); bystander words labelled `caller` (today: after the split cut; deferred veto: after its cut, or the whole utterance when it ends pending).\n\n| set | eligible |");
            for n in O2C_MODES {
                md.push_str(&format!(" {n} |"));
            }
            md.push_str(" deferred: end pending -> caller | bystander words labelled caller, today / deferred |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n");
            for e in far.iter() {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} | {} | {} / {} |\n",
                    e.set,
                    e.eligible,
                    e.hits[0],
                    e.hits[1],
                    e.hits[2],
                    e.hits[3],
                    e.end_caller,
                    e.caller_words[0],
                    e.caller_words[1]
                ));
            }
        }
        md.push('\n');
        md
    }

    static EV_PENDING: std::sync::Mutex<Vec<EvRec>> = std::sync::Mutex::new(Vec::new());
    static EV: std::sync::Mutex<Vec<EvRec>> = std::sync::Mutex::new(Vec::new());

    /// 7f (decision gg) modes of [`product_o2c`]: 0 today; 1 veto: a `caller` window whose scan
    /// completes (no budget stop) with no `caller` sub-window does not confirm, the window counts
    /// as `other` and the watch stays open; 2 narrow veto: the same, only when the window scored
    /// below threshold + [`NARROW_VETO_MARGIN`].
    /// 3 deferred veto (decision hh): the first such window is remembered with its fallback
    /// cut; a later window with a `caller` sub-window splits there; a second `caller` window
    /// whose scan finds none (completed or stopped on the budget) cuts at the remembered
    /// window's fallback; the utterance ending with the veto pending falls back to `caller` whole
    /// (the split is skipped as `not_speaking`; the barge is released at the end of speech).
    pub(super) const O2C_MODES: [&str; 4] = [
        "today",
        "veto",
        "narrow veto (< thr + 0.05)",
        "deferred veto (hh)",
    ];
    const NARROW_VETO_MARGIN: f32 = 0.05;

    /// Split-scan cost of one utterance under one mode: scans run, sub-window embeds the product
    /// would run (budget-bounded), their total ms and the longest scan's ms.
    #[derive(Default, Clone, Copy)]
    struct ScanStat {
        scans: usize,
        embeds: usize,
        ms: f64,
        max_ms: f64,
    }

    /// One mode's outcome for one utterance (7f): the confirming window `k`, the split's (lost,
    /// leaked) at the cut, U1's scores, the words lost with no confirm, whether the cut lands more
    /// than 1500 ms before the caller onset (a confirm on bystander-only audio, the n02 shape),
    /// and the scan cost.
    #[derive(Clone)]
    struct ModeRec {
        k: Option<usize>,
        split: Option<(usize, usize)>,
        u1: Vec<f32>,
        nosplit_lost: usize,
        early: bool,
        st: ScanStat,
        /// Mode 3: the utterance ended with the veto pending and falls back to `caller` whole.
        end_caller: bool,
        /// C4 (`NM_CAND` c4n<N>): no confirm, but the near-miss rule accepts the whole utterance
        /// as `caller` (no caller loss; every bystander word leaks). Always false by default.
        c4acc: bool,
    }

    type SubCache = std::collections::HashMap<usize, Vec<(usize, f32, f64)>>;
    /// A product other -> caller confirm: (k, cut, U1 window scores, the first `caller`
    /// sub-window start the cut backs off from, `None` for a fallback cut).
    type O2cConfirm = Option<(usize, usize, Vec<f32>, Option<usize>)>;

    /// The product gate with the watch on (chunk 6g): window 1 `other`, then the first window
    /// `k >= 2` scoring `caller`, or a near-miss window (`[0.30, thr)`) whose 750/125 scan finds a
    /// `caller` sub-window within the window's remaining verify budget. The cut is that
    /// sub-window's start minus 300 ms, never before the previous window's start (config A).
    /// `mode` (7f): see [`O2C_MODES`]. Returns (k, cut, U1 window scores = windows 1..k) and the
    /// scan cost; `cache` shares the sub-window scores of window `k` between modes.
    fn product_o2c(
        samples: &[f32],
        utt: &Utt,
        embedder: &Embedder,
        reference: &[f32],
        threshold: f32,
        mode: usize,
        cache: &mut SubCache,
    ) -> (O2cConfirm, ScanStat, bool) {
        let mut st = ScanStat::default();
        if utt.wins.first().is_none_or(|w| w.1 >= threshold) {
            return (None, st, false);
        }
        let u1 = |k: usize| utt.wins[..k].iter().map(|w| w.1).collect::<Vec<f32>>();
        let mut pending: Option<(usize, usize)> = None;
        for k in 1..utt.wins.len() {
            let (start, s, ms) = utt.wins[k];
            let lo = utt.wins[k - 1].0;
            let near = (P_NM_LO..threshold).contains(&s);
            if s < threshold && !near {
                continue;
            }
            let subs = cache.entry(k).or_insert_with(|| {
                subs_for(
                    samples,
                    lo,
                    start + WINDOW_MS,
                    (P_SUB, P_HOP),
                    embedder,
                    reference,
                )
            });
            let budget = VERIFY_MS - ms;
            let (mut spent, mut first, mut stopped, mut n) = (0.0, None, false, 0);
            for (t, sc, ems) in subs.iter() {
                if spent + ems > budget {
                    stopped = true;
                    break;
                }
                spent += ems;
                n += 1;
                if *sc >= threshold {
                    first = Some(*t);
                    break;
                }
            }
            st.scans += 1;
            st.embeds += n;
            st.ms += spent;
            st.max_ms = st.max_ms.max(spent);
            if near && first.is_none() {
                continue;
            }
            let veto = match mode {
                1 => true,
                2 => s < threshold + NARROW_VETO_MARGIN,
                _ => false,
            };
            if veto && first.is_none() && !stopped {
                continue;
            }
            if mode == 3 && first.is_none() {
                match pending {
                    Some((pk, plo)) => return (Some((pk, plo, u1(pk), None)), st, false),
                    None if !stopped => {
                        pending = Some((k, lo));
                        continue;
                    }
                    None => {}
                }
            }
            let cut = first.map_or(lo, |p| p.saturating_sub(P_BACK).max(lo));
            return (Some((k, cut, u1(k), first)), st, false);
        }
        (None, st, pending.is_some())
    }

    // ---- 7h (decision jj): the second cut on a split-born U2. ----

    /// `--second-cut`: simulate U2 of the shipped other -> caller split (mode 3) through its
    /// caller -> other change and cut, per rule of [`C2_RULES`].
    pub static SECOND_CUT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    /// `--takeover-db <dB>`: append a second bystander right after the caller's voiced end (no
    /// pause), at this gain relative to the (normalized) pieces.
    pub static TAKEOVER_DB: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    pub(super) const C2_RULES: [&str; 3] = [
        "today",
        "jj: pre-floor windows reset the run",
        "option 2: forward to the next closed quiet run",
    ];
    /// The other -> caller scan's own time after window `k` ends (eu-load 43752352272: 202 ms):
    /// U2's seed window ends here.
    const C2_SCAN_MS: usize = 200;
    const C2_PRE_ROLL_MS: usize = 300;

    struct C2Rec {
        set: String,
        takeover: bool,
        /// Caller reference words / bystander-2 reference words.
        b2_words: usize,
        /// Per rule: the second cut (scene ms), caller words cut out of U2, bystander-2 words left
        /// in U2.
        c2: [Option<usize>; 3],
        lost: [usize; 3],
        b2_left: [usize; 3],
    }
    static C2_PENDING: std::sync::Mutex<Vec<C2Rec>> = std::sync::Mutex::new(Vec::new());
    static C2: std::sync::Mutex<Vec<C2Rec>> = std::sync::Mutex::new(Vec::new());

    /// The second bystander: the head of the bystander clip (up to 2500 ms from its voiced onset)
    /// at `db`.
    fn takeover_clip(by: &Clip, db: f64) -> Vec<f32> {
        let g = 10f64.powf(db / 20.0) as f32;
        ms_to(&by.audio, by.on, (by.on + 2500).min(by.off))
            .into_iter()
            .map(|x| x * g)
            .collect()
    }

    fn pcm16(s: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(s.len() * 2);
        for x in s {
            out.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        out
    }

    /// Longest common subsequence length (words matched in order).
    fn lcs(a: &[String], b: &[String]) -> usize {
        let mut row = vec![0usize; b.len() + 1];
        for x in a {
            let mut prev = 0;
            for (j, y) in b.iter().enumerate() {
                let cur = row[j + 1];
                row[j + 1] = if x == y {
                    prev + 1
                } else {
                    row[j + 1].max(row[j])
                };
                prev = cur;
            }
        }
        row[b.len()]
    }

    /// U2's windows on U2's clock (origin = the split frame): the seed window `[0, seed end)`
    /// scored at the split (window `k`'s end + the scan), then the 1500 ms grid of U1 while the
    /// utterance lasts, clips with 300 ms of pre-roll. Returns (span, clip, score).
    fn u2_windows(
        p: &Placed,
        utt: &Utt,
        k: usize,
        origin: usize,
        embedder: &Embedder,
    ) -> Vec<((u32, u32), Vec<u8>, f32)> {
        u2_windows_raw(&p.samples, p.u_end, &p.reference, utt, k, origin, embedder)
    }

    fn u2_windows_raw(
        samples: &[f32],
        u_end: usize,
        reference: &[f32],
        utt: &Utt,
        k: usize,
        origin: usize,
        embedder: &Embedder,
    ) -> Vec<((u32, u32), Vec<u8>, f32)> {
        let mut out = Vec::new();
        let grid = utt.wins[k].0 + WINDOW_MS;
        let mut ends = vec![grid + C2_SCAN_MS];
        let mut e = grid + WINDOW_MS;
        while e <= u_end && ends.len() < MAX_WINDOWS {
            ends.push(e);
            e += WINDOW_MS;
        }
        for (i, end) in ends.into_iter().enumerate() {
            if end > u_end + C2_SCAN_MS || end <= origin {
                break;
            }
            let from = if i == 0 {
                origin
            } else {
                (end - WINDOW_MS - C2_PRE_ROLL_MS).max(origin)
            };
            let clip = slice(samples, from, end);
            let score = embedder
                .embed(clip)
                .map_or(0.0, |(v, _)| cosine(&v, reference));
            let start = if i == 0 { from } else { end - WINDOW_MS };
            out.push((
                ((start - origin) as u32, (end - origin) as u32),
                pcm16(clip),
                score,
            ));
        }
        out
    }

    /// The caller -> other watch of U2 (armed `caller` with no previous window, as
    /// `Msg::Inherit`): two `other` windows in a row confirm; rule 1 (jj) counts a window whose
    /// middle is before the floor as a reset. Returns the cut on U2's clock.
    fn u2_cut(
        wins: &[U2Win],
        floor: u32,
        threshold: f32,
        rule: usize,
        ctx: Scan<'_>,
    ) -> Option<u32> {
        u2_cut_span(wins, floor, threshold, rule, ctx).map(|(c, _)| c)
    }

    type U2Win = ((u32, u32), Vec<u8>, f32);
    /// Embedder and reference for the C7 `c7scan` veto (`None`: no scan available).
    type Scan<'a> = Option<(&'a Embedder, &'a [f32])>;

    /// The C7 switches of the caller -> other confirm.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct C7 {
        run: usize,
        nm: bool,
        scan: bool,
    }

    /// [`u2_cut`] with the confirming window's span (same clock as `wins`): (cut, (start, end)).
    /// The `NM_CAND` C7 switches apply; `c7scan` needs `ctx`.
    fn u2_cut_span(
        wins: &[U2Win],
        floor: u32,
        threshold: f32,
        rule: usize,
        ctx: Scan<'_>,
    ) -> Option<(u32, (u32, u32))> {
        let c = cand();
        let c7 = C7 {
            run: c.c7_run,
            nm: c.c7_nm,
            scan: c.c7_scan,
        };
        let mut scan = |clip: &[u8]| -> bool {
            let Some((embedder, reference)) = ctx else {
                return false;
            };
            let samples: Vec<f32> = clip
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[a, b]| f32::from(i16::from_le_bytes([a, b])) / 32768.0)
                .collect();
            let end = samples.len() / SAMPLES_PER_MS;
            subs_for(&samples, 0, end, (P_SUB, P_HOP), embedder, reference)
                .iter()
                .any(|(_, sc, _)| *sc >= threshold)
        };
        u2_cut_core(wins, floor, threshold, rule, c7, &mut scan)
    }

    /// The caller -> other watch. A window at or above the threshold (or, rule 1, whose middle is
    /// before the floor) resets the run. `c7.nm`: a near-miss window ([`NEAR_MISS_MIN_SCORE`],
    /// threshold) is neutral: it neither counts toward the `other` run nor resets it, and does not
    /// become `prev`. `c7.scan`: before a cut is confirmed, the 750/125 caller sub-window scan
    /// runs over the confirming windows' clips (`scan` = a sub-window at or above the threshold);
    /// a hit vetoes the cut and the run restarts (the scanned window becomes `prev`). The bench
    /// scan has no verify budget (optimistic).
    fn u2_cut_core(
        wins: &[U2Win],
        floor: u32,
        threshold: f32,
        rule: usize,
        c7: C7,
        scan: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Option<(u32, (u32, u32))> {
        let mut prev: Option<(u32, u32, Vec<u8>)> = None;
        let mut first: Option<(u32, u32, Vec<u8>)> = None;
        let mut run: Vec<&U2Win> = Vec::new();
        for win in wins {
            let ((s, e), clip, score) = win;
            let w = (*s, *e, clip.clone());
            let pre_floor = rule == 1 && s + (e - s) / 2 < floor;
            if *score >= threshold || pre_floor {
                run.clear();
                first = None;
                prev = Some(w);
                continue;
            }
            if c7.nm && *score >= NEAR_MISS_MIN_SCORE {
                continue;
            }
            if run.is_empty() {
                first = Some(w.clone());
            }
            run.push(win);
            if run.len() >= c7.run {
                if c7.scan && run.iter().any(|r| scan(&r.1)) {
                    run.clear();
                    first = None;
                    prev = Some(w);
                    continue;
                }
                return Some((
                    node_webrtc_rust_speech::speaker_gate::bench_caller_to_other_cut(
                        prev.take(),
                        first.take()?,
                        w,
                        Some(floor),
                        rule == 2,
                    ),
                    (*s, *e),
                ));
            }
        }
        None
    }

    fn second_cut_scene(
        p: &Placed,
        utt: &Utt,
        embedder: &Embedder,
        threshold: f32,
        cref: &[String],
        cache: &mut SubCache,
    ) {
        let (o, _, _) = product_o2c(&p.samples, utt, embedder, &p.reference, threshold, 3, cache);
        let Some((k, cut, _, found)) = o else { return };
        let origin = cut / FRAME_MS * FRAME_MS;
        let floor = found.map_or(300, |f| (f + 750).saturating_sub(origin)) as u32;
        let wins = u2_windows(p, utt, k, origin, embedder);
        let total = p.samples.len() / SAMPLES_PER_MS;
        let u2_end = (p.u_end + C2_PRE_ROLL_MS).min(total);
        let words = |end: usize| -> Vec<String> {
            recognize_words(slice(&p.samples, origin, end.max(origin + FRAME_MS)))
                .map(|(w, _)| w)
                .unwrap_or_default()
        };
        let full = lcs(cref, &words(u2_end));
        let b2ref: Vec<String> = p
            .by2
            .as_ref()
            .and_then(|(_, a)| recognize_words(a).map(|(w, _)| w))
            .unwrap_or_default();
        let mut rec = C2Rec {
            set: String::new(),
            takeover: p.by2.is_some(),
            b2_words: b2ref.len(),
            c2: [None; 3],
            lost: [0; 3],
            b2_left: [0; 3],
        };
        for r in 0..3 {
            let c2 = u2_cut(&wins, floor, threshold, r, Some((embedder, &p.reference)))
                .map(|c| origin + c as usize);
            let hyp = words(c2.unwrap_or(u2_end).min(u2_end));
            rec.c2[r] = c2;
            rec.lost[r] = full.saturating_sub(lcs(cref, &hyp));
            rec.b2_left[r] = lcs(&b2ref, &hyp);
        }
        C2_PENDING.lock().unwrap().push(rec);
    }

    /// The 7h table: per set and rule, U2s, U2 caller -> other splits, caller words cut out of U2
    /// (scenes), bystander-2 words left in U2 (takeover sets), takeovers split later / not split
    /// than today.
    pub fn second_cut_table() -> String {
        let recs = std::mem::take(&mut *C2.lock().unwrap());
        let mut sets: Vec<String> = Vec::new();
        for r in &recs {
            if !sets.contains(&r.set) {
                sets.push(r.set.clone());
            }
        }
        let mut md = String::from("\n## Chunk 7h: second cut on a split-born U2 (decision jj)\n\n| set | rule | U2s | caller -> other splits | caller words cut out of U2 (scenes) | bystander-2 words left in U2 / total | split later than today / not split |\n|---|---|---|---|---|---|---|\n");
        for set in &sets {
            let rs: Vec<&C2Rec> = recs.iter().filter(|r| &r.set == set).collect();
            let tk = rs.first().is_some_and(|r| r.takeover);
            for (ri, name) in C2_RULES.iter().enumerate() {
                let splits = rs.iter().filter(|r| r.c2[ri].is_some()).count();
                let lost: usize = rs.iter().map(|r| r.lost[ri]).sum();
                let lost_n = rs.iter().filter(|r| r.lost[ri] > 0).count();
                let (b2, b2t) = if tk {
                    (
                        rs.iter().map(|r| r.b2_left[ri]).sum::<usize>().to_string(),
                        rs.iter().map(|r| r.b2_words).sum::<usize>().to_string(),
                    )
                } else {
                    ("-".into(), "-".into())
                };
                let later = rs
                    .iter()
                    .filter(|r| matches!((r.c2[0], r.c2[ri]), (Some(a), Some(b)) if b > a))
                    .count();
                let none = rs
                    .iter()
                    .filter(|r| r.c2[0].is_some() && r.c2[ri].is_none())
                    .count();
                md.push_str(&format!(
                    "| {set} | {name} | {} | {splits} | {lost} ({lost_n}) | {b2} / {b2t} | {later} / {none} |\n",
                    rs.len()
                ));
            }
        }
        md
    }

    /// A caller-only utterance `[u, ue)` of `samples` (`clean` = the same timeline without the
    /// added bed, for the reference decode).
    fn caller_only_rec(
        set: &str,
        samples: &[f32],
        clean: &[f32],
        (u, ue): (usize, usize),
        embedder: &Embedder,
        reference: &[f32],
        threshold: f32,
    ) {
        let wins = score_windows(samples, u, ue, embedder, reference);
        let utt = Utt {
            wins,
            rechecks: Vec::new(),
        };
        let Some((cref, _)) = recognize_words(slice(clean, u, ue)) else {
            return;
        };
        let kept = |cut: usize| -> usize {
            let end = (cut + U2_HEAD_MS).min(ue);
            let (hyp, _) = recognize_words(slice(samples, cut, end)).unwrap_or_default();
            let (del, _) = head_align(&cref, &hyp);
            cref.len() - del.min(cref.len())
        };
        let floor_kept = kept(u);
        let mut cache = SubCache::default();
        let modes: Vec<ModeRec> = (0..O2C_MODES.len())
            .map(|m| {
                let (o, st, end_caller) =
                    product_o2c(samples, &utt, embedder, reference, threshold, m, &mut cache);
                match o {
                    Some((k, cut, u1, _)) => ModeRec {
                        k: Some(k),
                        split: Some((floor_kept.saturating_sub(kept(cut)), 0)),
                        u1,
                        nosplit_lost: 0,
                        early: false,
                        st,
                        end_caller,
                        c4acc: false,
                    },
                    // Window 1 `other` and no `caller` window: the whole utterance is dropped
                    // (a gate false reject under today's rule), counted above the floor.
                    None => {
                        let (c4acc, lost) = no_confirm(
                            &utt.wins,
                            threshold,
                            cand().c4,
                            end_caller,
                            floor_kept,
                            true,
                        );
                        ModeRec {
                            k: None,
                            split: None,
                            u1: Vec::new(),
                            nosplit_lost: lost,
                            early: false,
                            st,
                            end_caller,
                            c4acc,
                        }
                    }
                }
            })
            .collect();
        let m0 = modes[0].clone();
        EV.lock().unwrap().push(EvRec {
            set: set.to_string(),
            caller_only: true,
            u1: m0.u1,
            split: m0.split,
            upgrade_leak: 0,
            nosplit_lost: m0.nosplit_lost,
            ref_words: cref.len(),
            floor_gap: cref.len().saturating_sub(floor_kept),
            modes,
        });
    }

    fn load_bed(p: &Path) -> Result<Vec<f32>, String> {
        let bytes = fs::read(p).map_err(|e| format!("bed: {e}"))?;
        let f: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        Ok(f.as_chunks::<3>()
            .0
            .iter()
            .map(|c| (c[0] + c[1] + c[2]) / 3.0)
            .collect())
    }

    /// 6j set (1), LibriSpeech: whole utterances of each caller (VAD utterances of at least one
    /// window), same enrollment as the straddle set, Tocumen bed -12 dB and clean.
    pub fn caller_only_ls(
        embedder: &Embedder,
        root: &Path,
        bed: Option<&Path>,
        max_callers: usize,
        threshold: f32,
    ) -> Result<(), String> {
        let split = LS_SPLIT.get().map_or("test-clean", |s| s.as_str());
        let test = root.join(split);
        let mut spk: Vec<PathBuf> = fs::read_dir(&test)
            .map_err(|e| format!("{}: {e}", test.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        spk.sort();
        let bed16 = bed.map(load_bed).transpose()?;
        let mut conds: Vec<(String, Option<&[f32]>)> = Vec::new();
        if let Some(b) = &bed16 {
            conds.push((
                format!("LibriSpeech {split} caller-only, Tocumen bed -12 dB"),
                Some(b),
            ));
        }
        conds.push((format!("LibriSpeech {split} caller-only, clean"), None));
        for (cname, cbed) in &conds {
            for (ci, s) in spk.iter().take(max_callers).enumerate() {
                let mut files = Vec::new();
                wavs(s, &mut files)?;
                if files.len() < 1 + PIECES_PER_CALLER + 1 {
                    continue;
                }
                let e = load(&files[0])?;
                let Some((eon, _)) = voiced(&e) else { continue };
                let Some((reference, _)) = embedder.embed(slice(&e, eon, eon + 3000)) else {
                    continue;
                };
                for r in 0..CO_PIECES {
                    let Some(f) = files.get(1 + PIECES_PER_CALLER + r) else {
                        break;
                    };
                    let a = load(f)?;
                    let mut clean = vec![0f32; LEAD_MS * SAMPLES_PER_MS];
                    clean.extend_from_slice(slice(&a, 0, 15_000));
                    clean.extend(vec![0f32; TAIL_MS * SAMPLES_PER_MS]);
                    let mut s = clean.clone();
                    add_bed(&mut s, cbed.map(|b| (b, (ci * 6151 + r * 99991) % b.len())));
                    for (u, ue) in vad_utterances(&pcm48(&s))? {
                        if ue >= u + WINDOW_MS {
                            caller_only_rec(
                                cname,
                                &s,
                                &clean,
                                (u, ue),
                                embedder,
                                &reference,
                                threshold,
                            );
                        }
                    }
                }
                eprintln!("{cname} caller {ci} done");
            }
        }
        Ok(())
    }

    /// 6j set (1), Piper: 3 caller phrases of consecutive fixture scenes (one voice, the
    /// fixtures' babble bed) joined with 150 ms gaps into one utterance, scored against the
    /// first scene's enrollment; as is and with the Tocumen bed -12 dB added.
    pub fn caller_only_piper(
        embedder: &Embedder,
        dirs: &[PathBuf],
        bed: Option<&Path>,
        threshold: f32,
    ) -> Result<(), String> {
        let mut phrases: Vec<(Vec<f32>, Vec<f32>)> = Vec::new();
        for dir in dirs {
            let manifest = parse_json(
                &fs::read_to_string(dir.join("manifest.json"))
                    .map_err(|e| format!("manifest: {e}"))?,
            )?;
            let Some(Json::Arr(list)) = manifest.get("scenes") else {
                return Err("manifest has no scenes".into());
            };
            for scene in list {
                let id = text(scene, "id").unwrap_or_default();
                let Some(Json::Arr(segs)) = scene.get("segments") else {
                    continue;
                };
                let role = |s: &Json| text(s, "role").unwrap_or_default();
                let is_enroll = |s: &Json| matches!(s.get("enroll"), Some(Json::Bool(true)));
                let (Some(e), Some(c)) = (
                    segs.iter().find(|s| is_enroll(s)),
                    segs.iter().find(|s| role(s) == "main" && !is_enroll(s)),
                ) else {
                    continue;
                };
                let (Some(e), Some(cs)) = (span_of(e), span_of(c)) else {
                    continue;
                };
                let samples = read_wav(&dir.join("scenes").join(format!("{id}.16k.wav")))?;
                let (reference, _) = embedder
                    .embed(slice(&samples, e.on, e.off))
                    .ok_or("enroll embed")?;
                phrases.push((ms_to(&samples, cs.on, cs.off), reference));
            }
        }
        let bed16 = bed.map(load_bed).transpose()?;
        let n = phrases.len();
        let mut conds: Vec<(&str, Option<&[f32]>)> =
            vec![("Piper caller-only, fixture babble", None)];
        if let Some(b) = &bed16 {
            conds.push((
                "Piper caller-only, fixture babble + Tocumen bed -12 dB",
                Some(b),
            ));
        }
        for (cname, cbed) in conds {
            for i in 0..n {
                let mut clean = vec![0f32; LEAD_MS * SAMPLES_PER_MS];
                for j in 0..3 {
                    if j > 0 {
                        clean.extend(vec![0f32; CO_GAP_MS * SAMPLES_PER_MS]);
                    }
                    clean.extend_from_slice(&phrases[(i + j) % n].0);
                }
                clean.extend(vec![0f32; TAIL_MS * SAMPLES_PER_MS]);
                let mut s = clean.clone();
                add_bed(&mut s, cbed.map(|b| (b, (i * 7919) % b.len())));
                for (u, ue) in vad_utterances(&pcm48(&s))? {
                    if ue >= u + WINDOW_MS {
                        caller_only_rec(
                            cname,
                            &s,
                            &clean,
                            (u, ue),
                            embedder,
                            &phrases[i].1,
                            threshold,
                        );
                    }
                }
            }
            eprintln!("{cname} done ({n} utterances)");
        }
        Ok(())
    }

    // ---- caller-reject-rate chunk 1: per-utterance caller loss (`--utterance-loss`). ----

    /// `--utterance-loss`: LibriSpeech only, product enrollment, shipped rules (deferred veto +
    /// decision bb), outcome and cause per VAD utterance (`NEAR_MISS_BENCH.md`, "Per-utterance
    /// caller loss").
    pub static UTT_LOSS: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    const UL_TEST_SAME: usize = 15;
    const UL_TEST_CROSS: usize = 10;
    const UL_ORACLE_MS: usize = 30_000;

    const UL_BYSTANDERS: usize = 3;
    const UL_BY_FILES: usize = 3;
    const UL_CAUSES: [&str; 5] = [
        "bad enrollment",
        "noise",
        "short utterance",
        "speaker the model handles badly",
        "unexplained",
    ];

    #[derive(Clone, Copy, PartialEq)]
    enum UlOut {
        Caller,
        Dropped,
        Partly,
        Unknown,
    }

    struct UlRec {
        set: String,
        mode: &'static str,
        speaker: String,
        out: UlOut,
        lost: usize,
        cause: Option<usize>,
        c2o: Option<(usize, usize)>,
    }

    #[derive(Default)]
    struct UlFar {
        set: String,
        mode: &'static str,
        windows: usize,
        hits: usize,
        utts: usize,
        accepted: usize,
        pairs: Vec<(usize, usize)>,
    }

    use super::cand::{c4_scores, cand};

    /// An utterance with no other -> caller confirm: (accepted whole by C4, caller words lost).
    /// `first_other_only`: a caller-only utterance counts as lost only when window 1 is `other`.
    fn no_confirm(
        wins: &[Win],
        threshold: f32,
        c4: usize,
        end_caller: bool,
        floor_kept: usize,
        first_other_only: bool,
    ) -> (bool, usize) {
        let scores: Vec<f32> = wins.iter().map(|w| w.1).collect();
        let acc = !end_caller && c4_scores(&scores, threshold, c4);
        let dropped = !end_caller
            && !acc
            && (!first_other_only || wins.first().is_some_and(|w| w.1 < threshold));
        (acc, if dropped { floor_kept } else { 0 })
    }

    fn c4_accept(wins: &[Win], threshold: f32) -> bool {
        let scores: Vec<f32> = wins.iter().map(|w| w.1).collect();
        c4_scores(&scores, threshold, cand().c4)
    }

    /// Voiced ms consumed before the gate is enrolled: (set, mode, ms).
    static UL_ENR: std::sync::Mutex<Vec<(String, &'static str, usize)>> =
        std::sync::Mutex::new(Vec::new());

    /// Speakers whose pre-test files could not enroll the gate: (set, mode).
    static UL_SKIP: std::sync::Mutex<Vec<(String, &'static str)>> =
        std::sync::Mutex::new(Vec::new());

    static UL: std::sync::Mutex<Vec<UlRec>> = std::sync::Mutex::new(Vec::new());
    static UL_FAR: std::sync::Mutex<Vec<UlFar>> = std::sync::Mutex::new(Vec::new());

    struct Judged {
        out: UlOut,
        lost: usize,
        wins: usize,
        /// A caller -> other split cut the part kept as caller: (class, caller words cut out).
        /// Classed by the confirming window ([`c2o_class`]): 0 = confirmed by a bed-tail window,
        /// 1 = confirmed by post-truncation audio (artefact), 2 = confirmed mid-speech.
        c2o: Option<(usize, usize)>,
        /// The utterance's window scores (U1 grid), for the lost-utterance TSV.
        scores: Vec<f32>,
    }

    /// Class of a caller -> other split by its confirming window `(cs, ce)` (scene ms): 1 when
    /// the scene was truncated at 15 s and the window ends after the truncation point; else 0
    /// when the window's middle is at or after the clean voiced end `v_end` (the window holds
    /// mostly the bed after the caller stopped); else 2 (confirmed mid-speech).
    fn c2o_class(trunc: bool, v_end: usize, (cs, ce): (usize, usize)) -> usize {
        if trunc && ce > LEAD_MS + 15_000 {
            1
        } else if cs + (ce - cs) / 2 >= v_end {
            0
        } else {
            2
        }
    }

    /// Caller -> other watch on windows `from..` of an utterance kept as caller: the first window
    /// is armed `caller` (floor 0, rule 0, as `Msg::Inherit`); only an utterance whose window 1 is
    /// `caller` has this watch (an upgrade or a deferred end-of-speech upgrade has none). Returns
    /// the cut and the confirming window's span (scene ms).
    fn ul_c2o_whole(
        samples: &[f32],
        utt: &Utt,
        threshold: f32,
        ctx: Scan<'_>,
    ) -> Option<(usize, (usize, usize))> {
        let wins: Vec<((u32, u32), Vec<u8>, f32)> = utt
            .wins
            .iter()
            .map(|(start, score, _)| {
                let clip = slice(
                    samples,
                    start.saturating_sub(PRE_ROLL_MS),
                    start + WINDOW_MS,
                );
                (
                    (*start as u32, (*start + WINDOW_MS) as u32),
                    pcm16(clip),
                    *score,
                )
            })
            .collect();
        u2_cut_span(&wins, 0, threshold, 0, ctx)
            .map(|(c, (cs, ce))| (c as usize, (cs as usize, ce as usize)))
    }

    /// The shipped gate on one utterance `[u, ue)` (>= 1 window): window 1 `caller` keeps it;
    /// otherwise product other -> caller (deferred veto, mode 3) with split rule bb. Then the
    /// caller -> other watch runs on the part kept as caller (whole utterance: armed from window
    /// 1, floor 0, rule 0; split-born U2: the 7h clock, floor from the scan, rule 1 = jj; upgrade:
    /// from the confirming window). Words lost are measure A above the floor (cut at `u`).
    fn ul_judge(
        samples: &[f32],
        clean: &[f32],
        (u, ue): (usize, usize),
        embedder: &Embedder,
        reference: &[f32],
        threshold: f32,
        dump: bool,
    ) -> Judged {
        let wins = score_windows(samples, u, ue, embedder, reference);
        let n = wins.len();
        if n == 0 {
            return Judged {
                out: UlOut::Unknown,
                lost: 0,
                wins: 0,
                c2o: None,
                scores: Vec::new(),
            };
        }
        let utt = Utt {
            wins,
            rechecks: Vec::new(),
        };
        let total = samples.len() / SAMPLES_PER_MS;
        let first_caller = utt.wins[0].1 >= threshold;
        let mut cache = SubCache::default();
        let (o, _st, end_caller) = if first_caller {
            (None, ScanStat::default(), true)
        } else {
            product_o2c(samples, &utt, embedder, reference, threshold, 3, &mut cache)
        };
        let words = |a: usize, b: usize| -> Vec<String> {
            recognize_words(slice(samples, a, b.max(a + FRAME_MS)))
                .map(|(w, _)| w)
                .unwrap_or_default()
        };
        let (cref, _) = recognize_words(slice(clean, u, ue)).unwrap_or_default();
        let kept = |cut: usize| -> usize {
            let end = (cut + U2_HEAD_MS).min(ue);
            let (hyp, _) = recognize_words(slice(samples, cut, end)).unwrap_or_default();
            let (del, _) = head_align(&cref, &hyp);
            cref.len() - del.min(cref.len())
        };
        let finish =
            |lost: usize, c2: Option<(usize, usize, (usize, usize))>, dropped: bool| -> Judged {
                // c2 = (origin of the kept part, cut, confirming window span), scene ms: caller words
                // cut out of the kept part.
                let extra = c2.map_or(0, |(origin, cut, _)| {
                    let end = (ue + C2_PRE_ROLL_MS).min(total);
                    lcs(&cref, &words(origin, end))
                        .saturating_sub(lcs(&cref, &words(origin, cut.min(end))))
                });
                let cls = c2.map(|(_, _, span)| {
                    let trunc = clean.len() >= (LEAD_MS + 15_000 + TAIL_MS) * SAMPLES_PER_MS;
                    let v_end = voiced(clean).map_or(0, |v| v.1);
                    c2o_class(trunc, v_end, span)
                });
                let lost = lost + extra;
                Judged {
                    out: if dropped {
                        UlOut::Dropped
                    } else if lost > 0 {
                        UlOut::Partly
                    } else {
                        UlOut::Caller
                    },
                    lost,
                    wins: n,
                    c2o: cls.map(|c| (c, extra)),
                    scores: utt.wins.iter().map(|w| w.1).collect(),
                }
            };
        match o {
            Some((k, cut, u1, found)) => {
                if dump {
                    dump_split_raw(embedder, samples, reference, &utt, k, threshold);
                }
                if u1.iter().any(|x| *x < 0.20) {
                    let lost = kept(u).saturating_sub(kept(cut));
                    let origin = cut / FRAME_MS * FRAME_MS;
                    let floor = found.map_or(300, |f| (f + 750).saturating_sub(origin)) as u32;
                    let w2 = u2_windows_raw(samples, ue, reference, &utt, k, origin, embedder);
                    let c2 = u2_cut_span(&w2, floor, threshold, 1, Some((embedder, reference)))
                        .map(|(c, (cs, ce))| {
                            (
                                origin,
                                origin + c as usize,
                                (origin + cs as usize, origin + ce as usize),
                            )
                        });
                    finish(lost, c2, false)
                } else {
                    finish(0, None, false)
                }
            }
            None if end_caller && first_caller => {
                let c2 = ul_c2o_whole(samples, &utt, threshold, Some((embedder, reference)))
                    .map(|(c, span)| (u, c, span));
                finish(0, c2, false)
            }
            None if end_caller => finish(0, None, false),
            None if c4_accept(&utt.wins, threshold) => finish(0, None, false),
            None => finish(kept(u), None, true),
        }
    }

    /// Appends a lost utterance's window scores to the TSV at `NM_UL_WINDOWS` (never committed):
    /// set, mode, speaker, outcome, words lost, comma-separated window scores.
    fn ul_windows_tsv(set: &str, mode: &str, speaker: &str, j: &Judged) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let Some(path) = std::env::var_os("NM_UL_WINDOWS") else {
            return;
        };
        let _g = LOCK.lock().unwrap();
        let scores: Vec<String> = j.scores.iter().map(|x| format!("{x:.3}")).collect();
        let line = format!(
            "{set}\t{mode}\t{speaker}\t{}\t{}\t{}\n",
            if j.out == UlOut::Dropped {
                "dropped"
            } else {
                "partly"
            },
            j.lost,
            scores.join(",")
        );
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn ul_scene(a: &[f32], bed: Option<(&[f32], usize)>) -> (Vec<f32>, Vec<f32>) {
        let mut clean = vec![0f32; LEAD_MS * SAMPLES_PER_MS];
        clean.extend_from_slice(slice(a, 0, 15_000));
        clean.extend(vec![0f32; TAIL_MS * SAMPLES_PER_MS]);
        let mut s = clean.clone();
        add_bed(&mut s, bed);
        (s, clean)
    }

    /// Product enrollment: whole voiced utterances appended until >= the enrollment span, one embed
    /// of the whole buffer (C2: mean of per-utterance unit embeddings; C3: reject a poor one).
    /// The bed (when set) is on the enrollment audio too. Returns (reference, files used, voiced
    /// ms consumed).
    fn ul_enroll(
        files: &[PathBuf],
        embedder: &Embedder,
        bed: Option<&[f32]>,
        salt: usize,
    ) -> Result<Option<(Vec<f32>, usize, usize)>, String> {
        super::cand::enroll_product(
            files.len(),
            |i| {
                let a = load(&files[i])?;
                let Some((on, off)) = voiced(&a) else {
                    return Ok(None);
                };
                let mut u = slice(&a, on, off).to_vec();
                add_bed(&mut u, bed.map(|b| (b, (salt + i * 7919) % b.len())));
                Ok(Some(u))
            },
            |x| embedder.embed(x).map(|(e, _)| e),
            &cand(),
        )
    }

    /// Oracle reference: mean of the unit embeddings of about 30 s of other same-session
    /// utterances (same bed condition).
    fn ul_oracle(
        files: &[PathBuf],
        embedder: &Embedder,
        bed: Option<&[f32]>,
        salt: usize,
    ) -> Result<Option<Vec<f32>>, String> {
        let (mut acc, mut ms): (Vec<f32>, usize) = (Vec::new(), 0);
        for (i, f) in files.iter().enumerate() {
            if ms >= UL_ORACLE_MS {
                break;
            }
            let a = load(f)?;
            let Some((on, off)) = voiced(&a) else {
                continue;
            };
            let mut c = slice(&a, on, off.min(on + 20_000)).to_vec();
            add_bed(&mut c, bed.map(|b| (b, (salt + i * 104729) % b.len())));
            let Some((e, _)) = embedder.embed(&c) else {
                continue;
            };
            let n = e.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
            if acc.is_empty() {
                acc = vec![0.0; e.len()];
            }
            for (a, x) in acc.iter_mut().zip(&e) {
                *a += x / n;
            }
            ms += off.min(on + 20_000) - on;
        }
        Ok((!acc.is_empty()).then_some(acc))
    }

    struct UlScene {
        s: Vec<f32>,
        clean: Vec<f32>,
        utts: Vec<(usize, usize)>,
    }

    fn ul_far_slot<'a>(far: &'a mut Vec<UlFar>, set: &str, mode: &'static str) -> &'a mut UlFar {
        let i = match far.iter().position(|e| e.set == set && e.mode == mode) {
            Some(i) => i,
            None => {
                far.push(UlFar {
                    set: set.to_string(),
                    mode,
                    ..Default::default()
                });
                far.len() - 1
            }
        };
        &mut far[i]
    }

    pub fn utterance_loss(
        embedder: &Embedder,
        root: &Path,
        bed: Option<&Path>,
        max_callers: usize,
        threshold: f32,
    ) -> Result<(), String> {
        let split = LS_SPLIT.get().map_or("test-clean", |s| s.as_str());
        let top = root.join(split);
        let mut spk: Vec<PathBuf> = fs::read_dir(&top)
            .map_err(|e| format!("{}: {e}", top.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        spk.sort();
        // Per speaker: chapters (sorted), each its sorted wav list.
        let mut chapters: Vec<Vec<Vec<PathBuf>>> = Vec::new();
        for s in &spk {
            let mut ch: Vec<PathBuf> = fs::read_dir(s)
                .map_err(|e| format!("{}: {e}", s.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir())
                .collect();
            ch.sort();
            let mut v = Vec::new();
            for c in ch {
                let mut f = Vec::new();
                wavs(&c, &mut f)?;
                if f.len() >= 8 {
                    v.push(f);
                }
            }
            chapters.push(v);
        }
        let n = spk.len();
        let bed16 = bed.map(load_bed).transpose()?;
        let mut conds: Vec<(String, Option<&[f32]>)> = Vec::new();
        if let Some(b) = &bed16 {
            conds.push((format!("LibriSpeech {split}, Tocumen bed -12 dB"), Some(b)));
        }
        conds.push((format!("LibriSpeech {split}, clean"), None));
        for (cname, cbed) in &conds {
            for ci in 0..max_callers.min(n) {
                let sid = spk[ci]
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let Some(ch_a) = chapters[ci].first() else {
                    continue;
                };
                let salt = ci * 6151;
                for mode in ["same-session", "cross-session"] {
                    // Decision rr (i): the test utterances are the LAST files of the test chapter, the
                    // same for every candidate; enrollment draws only from the files before them.
                    let (enroll_src, rest): (&[PathBuf], &Vec<PathBuf>) = if mode == "same-session"
                    {
                        (&ch_a[..ch_a.len().saturating_sub(UL_TEST_SAME)], ch_a)
                    } else {
                        match chapters[ci].get(1) {
                            Some(b) => (&ch_a[..], b),
                            None => continue,
                        }
                    };
                    let set = cname.to_string();
                    let t_n = if mode == "same-session" {
                        UL_TEST_SAME
                    } else {
                        UL_TEST_CROSS
                    };
                    let split_at = rest.len().saturating_sub(t_n);
                    let Some((reference, _enr_files, enr_ms)) =
                        ul_enroll(enroll_src, embedder, *cbed, salt)?
                    else {
                        UL_SKIP.lock().unwrap().push((set.clone(), mode));
                        continue;
                    };
                    UL_ENR
                        .lock()
                        .unwrap()
                        .push((cname.to_string(), mode, enr_ms));
                    if std::env::var_os("NM_ENROLL_ONLY").is_some() {
                        continue;
                    }
                    let tests: Vec<&PathBuf> = rest.iter().skip(split_at).collect();
                    // Oracle pool: the (up to) 40 files just before the tests, the same for every
                    // candidate (it may overlap the enrollment files: attribution only).
                    let pool: Vec<PathBuf> = rest[..split_at]
                        .iter()
                        .skip(split_at.saturating_sub(40))
                        .cloned()
                        .collect();
                    let Some(oracle) = ul_oracle(&pool, embedder, *cbed, salt + 17)? else {
                        continue;
                    };
                    let mut scenes: Vec<UlScene> = Vec::new();
                    for (r, f) in tests.iter().enumerate() {
                        let a = load(f)?;
                        let (s, clean) =
                            ul_scene(&a, cbed.map(|b| (b, (ci * 6151 + r * 99991) % b.len())));
                        let utts = vad_utterances(&pcm48(&s))?;
                        scenes.push(UlScene { s, clean, utts });
                    }
                    let mut p10: Option<f32> = None;
                    for sc in &scenes {
                        for &(u, ue) in &sc.utts {
                            if ue < u + WINDOW_MS {
                                UL.lock().unwrap().push(UlRec {
                                    set: set.clone(),
                                    mode,
                                    speaker: sid.clone(),
                                    out: UlOut::Unknown,
                                    lost: 0,
                                    cause: None,
                                    c2o: None,
                                });
                                continue;
                            }
                            let dump = std::env::var_os("NM_SPLIT_DUMP").is_some();
                            let j = ul_judge(
                                &sc.s,
                                &sc.clean,
                                (u, ue),
                                embedder,
                                &reference,
                                threshold,
                                dump,
                            );
                            let mut cause = None;
                            if matches!(j.out, UlOut::Dropped | UlOut::Partly) {
                                let rec = |s: &[f32], r: &[f32]| {
                                    ul_judge(s, &sc.clean, (u, ue), embedder, r, threshold, false)
                                        .out
                                        == UlOut::Caller
                                };
                                cause = Some(if rec(&sc.s, &oracle) {
                                    0
                                } else if cbed.is_some() && rec(&sc.clean, &reference) {
                                    1
                                } else if j.wins <= 2 {
                                    2
                                } else {
                                    let p = *p10.get_or_insert_with(|| {
                                        let mut v: Vec<f64> = Vec::new();
                                        for s2 in &scenes {
                                            for &(a, b) in &s2.utts {
                                                if b >= a + WINDOW_MS {
                                                    for w in score_windows(
                                                        &s2.s, a, b, embedder, &oracle,
                                                    ) {
                                                        v.push(f64::from(w.1));
                                                    }
                                                }
                                            }
                                        }
                                        v.sort_by(|x, y| x.total_cmp(y));
                                        percentile(&v, 0.10) as f32
                                    });
                                    if p < threshold {
                                        3
                                    } else {
                                        4
                                    }
                                });
                            }
                            if cause.is_some() {
                                ul_windows_tsv(&set, mode, &sid, &j);
                            }
                            UL.lock().unwrap().push(UlRec {
                                set: set.clone(),
                                mode,
                                speaker: sid.clone(),
                                out: j.out,
                                lost: j.lost,
                                cause,
                                c2o: j.c2o,
                            });
                        }
                    }
                    // Bystander baselines against this reference: other speakers' utterances.
                    for b in 0..UL_BYSTANDERS {
                        let bs = (ci + 1 + b * 3) % n;
                        if bs == ci {
                            continue;
                        }
                        let Some(bch) = chapters[bs].first() else {
                            continue;
                        };
                        let (mut w_n, mut w_hit) = (0usize, 0usize);
                        let (mut u_n, mut u_acc) = (0usize, 0usize);
                        for (r, f) in bch.iter().skip(ci % 3).take(UL_BY_FILES).enumerate() {
                            let a = load(f)?;
                            let (s, _) = ul_scene(
                                &a,
                                cbed.map(|bd| (bd, (ci * 31 + b * 7907 + r * 13) % bd.len())),
                            );
                            for (u, ue) in vad_utterances(&pcm48(&s))? {
                                if ue < u + WINDOW_MS {
                                    continue;
                                }
                                let wins = score_windows(&s, u, ue, embedder, &reference);
                                w_n += wins.len();
                                w_hit += wins.iter().filter(|w| w.1 >= threshold).count();
                                let utt = Utt {
                                    wins,
                                    rechecks: Vec::new(),
                                };
                                u_n += 1;
                                let acc = utt.wins[0].1 >= threshold
                                    || c4_accept(&utt.wins, threshold)
                                    || {
                                        let mut cache = SubCache::default();
                                        let (o, _, end) = product_o2c(
                                            &s, &utt, embedder, &reference, threshold, 3,
                                            &mut cache,
                                        );
                                        o.is_some() || end
                                    };
                                u_acc += usize::from(acc);
                            }
                        }
                        let mut far = UL_FAR.lock().unwrap();
                        let e = ul_far_slot(&mut far, &set, mode);
                        e.windows += w_n;
                        e.hits += w_hit;
                        e.utts += u_n;
                        e.accepted += u_acc;
                        e.pairs.push((w_n, w_hit));
                    }
                }
                eprintln!("{cname} speaker {sid} ({ci}) done");
            }
        }
        Ok(())
    }

    pub fn utterance_loss_tables() -> String {
        let recs = std::mem::take(&mut *UL.lock().unwrap());
        let far = std::mem::take(&mut *UL_FAR.lock().unwrap());
        let mut keys: Vec<(String, &'static str)> = Vec::new();
        for r in &recs {
            if !keys.iter().any(|(s, m)| *s == r.set && *m == r.mode) {
                keys.push((r.set.clone(), r.mode));
            }
        }
        let mut md = String::from("## Per-utterance caller loss (caller-reject-rate, chunk 1)\n\nShipped rules (`split_on_speaker_change` on: window 1 `other`, then product other -> caller with the deferred veto and split rule bb). Enrollment is the product's: whole voiced utterances appended until 3000 ms, one embed of the buffer (with the bed, when the row has one). Words are measure A above the floor of the same utterance. Outcome: `dropped` = window 1 `other` and no confirm; `partly lost` = a split cut caller words; `unknown` = VAD utterance under 1.5 s (fails open).\n\n| set | mode | utterances (>= 1.5 s) | unknown (< 1.5 s) | dropped | partly lost | caller->other splits: total | (a) confirmed by bed-tail window: splits / words | (b) confirmed by post-truncation audio: splits / words | (c) confirmed mid-speech: splits / words | lost utterances % | words lost above floor | words lost per speaker: median / max |");
        for c in UL_CAUSES {
            md.push_str(&format!(" {c} |"));
        }
        md.push_str(" top 5 speakers (words lost, utterances lost) | top 5 share of words |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
        for _ in UL_CAUSES {
            md.push_str(" --- |");
        }
        md.push_str(" --- | --- |\n");
        for (set, mode) in &keys {
            let rs: Vec<&UlRec> = recs
                .iter()
                .filter(|r| r.set == *set && r.mode == *mode)
                .collect();
            let known: Vec<&&UlRec> = rs.iter().filter(|r| r.out != UlOut::Unknown).collect();
            let dropped = known.iter().filter(|r| r.out == UlOut::Dropped).count();
            let partly = known.iter().filter(|r| r.out == UlOut::Partly).count();
            let words: usize = known.iter().map(|r| r.lost).sum();
            let mut c2c = [(0usize, 0usize); 3];
            for r in &known {
                if let Some((c, w)) = r.c2o {
                    c2c[c].0 += 1;
                    c2c[c].1 += w;
                }
            }
            let c2n = c2c[0].0 + c2c[1].0 + c2c[2].0;
            let mut per: std::collections::BTreeMap<&str, usize> = Default::default();
            for r in &known {
                *per.entry(r.speaker.as_str()).or_default() += r.lost;
            }
            let mut pv: Vec<usize> = per.values().copied().collect();
            pv.sort();
            let med = if pv.is_empty() {
                0.0
            } else if pv.len() % 2 == 1 {
                pv[pv.len() / 2] as f64
            } else {
                (pv[pv.len() / 2 - 1] + pv[pv.len() / 2]) as f64 / 2.0
            };
            let mx = pv.last().copied().unwrap_or(0);
            let mut cs = [0usize; 5];
            for r in &known {
                if let Some(c) = r.cause {
                    cs[c] += 1;
                }
            }
            let mut by: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
            for r in &known {
                if matches!(r.out, UlOut::Dropped | UlOut::Partly) {
                    let e = by.entry(r.speaker.as_str()).or_default();
                    e.0 += r.lost;
                    e.1 += 1;
                }
            }
            let mut sp: Vec<(&str, (usize, usize))> = by.into_iter().collect();
            sp.sort_by_key(|a| std::cmp::Reverse(a.1));
            let top: usize = sp.iter().take(5).map(|x| x.1 .0).sum();
            let top_s = sp
                .iter()
                .take(5)
                .map(|(s, (w, u))| format!("{s}: {w}, {u}"))
                .collect::<Vec<_>>()
                .join("; ");
            md.push_str(&format!(
                "| {set} | {mode} | {} | {} | {dropped} | {partly} | {c2n} | {} / {} | {} / {} | {} / {} | {:.1} | {words} | {med:.1} / {mx} |",
                known.len(),
                rs.len() - known.len(),
                c2c[0].0,
                c2c[0].1,
                c2c[1].0,
                c2c[1].1,
                c2c[2].0,
                c2c[2].1,
                100.0 * (dropped + partly) as f64 / known.len().max(1) as f64
            ));
            for c in cs {
                md.push_str(&format!(" {c} |"));
            }
            md.push_str(&format!(
                " {top_s} | {:.0} % |\n",
                if words > 0 {
                    100.0 * top as f64 / words as f64
                } else {
                    0.0
                }
            ));
        }
        md.push_str("\nCause = the first counterfactual that recovers the utterance, in order: oracle reference (mean of embeddings of about 30 s of other same-session utterances), bed removed from the test utterance only, short utterance (<= 2 windows), speaker whose oracle-reference window p10 is below the threshold, else unexplained. Cause counts are over lost utterances.\n\n### Bystander baselines with the same harness (other speakers' utterances against this reference)\n\n| set | mode | windows | window FAR % | pairs | pairs >= 5 % | pairs >= 20 % | bystander-only utterances (>= 1.5 s) | accepted (window 1 caller, or confirm, or veto pending) |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for e in &far {
            let pc = |x: f64| {
                e.pairs
                    .iter()
                    .filter(|(w, h)| *w > 0 && 100.0 * *h as f64 / *w as f64 >= x)
                    .count()
            };
            md.push_str(&format!(
                "| {} | {} | {} | {:.2} | {} | {} | {} | {} | {} |\n",
                e.set,
                e.mode,
                e.windows,
                100.0 * e.hits as f64 / e.windows.max(1) as f64,
                e.pairs.len(),
                pc(5.0),
                pc(20.0),
                e.utts,
                e.accepted
            ));
        }
        md.push_str("\nCandidate `NM_CAND`=");
        md.push_str(&std::env::var("NM_CAND").unwrap_or_default());
        md.push_str(" enrollment voiced ms before the gate is enrolled (median / p90 / max over speakers):\n\n");
        let enr = std::mem::take(&mut *UL_ENR.lock().unwrap());
        let skip = std::mem::take(&mut *UL_SKIP.lock().unwrap());
        for (set, mode) in &keys {
            let sk = skip.iter().filter(|e| e.0 == *set && e.1 == *mode).count();
            md.push_str(&format!(
                "- {set}, {mode}: speakers skipped (no enrollment from the files before the test set): {sk}\n"
            ));
            let mut v: Vec<usize> = enr
                .iter()
                .filter(|e| e.0 == *set && e.1 == *mode)
                .map(|e| e.2)
                .collect();
            v.sort_unstable();
            if let (Some(m), Some(x)) = (v.get(v.len() / 2), v.last()) {
                md.push_str(&format!(
                    "- {set}, {mode}: {m} / {} / {x}\n",
                    v[(v.len() * 9 / 10).min(v.len() - 1)]
                ));
            }
        }
        md.push('\n');
        md
    }

    type Rule = (String, Box<dyn Fn(&[f32]) -> bool>);

    /// Candidate rules on U1's window scores (all below the threshold): `true` = split.
    fn ev_rules() -> Vec<Rule> {
        let min = |u: &[f32]| u.iter().cloned().fold(f32::MAX, f32::min);
        let mut v: Vec<Rule> = vec![
            ("today: always split".into(), Box::new(|_| true)),
            ("never split (always upgrade)".into(), Box::new(|_| false)),
            (
                "R2: U1 has >= 2 other windows".into(),
                Box::new(|u: &[f32]| u.len() >= 2),
            ),
        ];
        for x in [0.20f32, 0.25, 0.30, 0.35, 0.40] {
            let tag = if (x - 0.30).abs() < 1e-6 { "R1" } else { "R1'" };
            v.push((
                format!("{tag}: a U1 window < {x:.2}"),
                Box::new(move |u: &[f32]| min(u) < x),
            ));
            let tag = if (x - 0.30).abs() < 1e-6 {
                "R3 (= R4)"
            } else {
                "R3'"
            };
            v.push((
                format!("{tag}: a U1 window < {x:.2}, or >= 2 other windows"),
                Box::new(move |u: &[f32]| min(u) < x || u.len() >= 2),
            ));
        }
        // 7i (decision kk): bb's < 0.20 (and 0.25 for reference), or a long `other` run in U1.
        for x in [0.20f32, 0.25] {
            let tag = if (x - 0.20).abs() < 1e-6 { "R5" } else { "R5'" };
            for n in [3usize, 4, 5] {
                v.push((
                    format!("{tag}({n}): a U1 window < {x:.2}, or >= {n} other windows"),
                    Box::new(move |u: &[f32]| min(u) < x || u.len() >= n),
                ));
            }
        }
        v
    }

    /// The 6j tables (decision aa), lost words above the STT floor. Per record, the floor is
    /// measure A's miss with no gate (cut at the caller onset, or at U1's start for caller-only).
    /// A split loses `floor kept - kept at the cut`; an upgrade loses nothing above the floor (the
    /// whole utterance is the caller final); a whole-utterance drop (window 1 `other`, no confirm)
    /// loses the floor-kept words under every rule and is reported apart. Rows are ranked by
    /// split-dependent lost words over both sets, then leaked words. `NM_EV_TSV` (default
    /// /tmp/sg-6j/ev.tsv) gets one line per utterance.
    pub fn evidence_tables() -> String {
        let recs = std::mem::take(&mut *EV.lock().unwrap());
        let rules = ev_rules();
        let mut sets: Vec<(String, bool)> = Vec::new();
        for r in &recs {
            if !sets.iter().any(|(s, _)| *s == r.set) {
                sets.push((r.set.clone(), r.caller_only));
            }
        }
        let scores = |u: &[f32]| {
            u.iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut tsv = String::new();
        for r in &recs {
            let (sl, sk) = r.split.unwrap_or((0, 0));
            tsv.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{sl}\t{sk}\t{}\t{}\n",
                r.set,
                u8::from(r.caller_only),
                r.ref_words,
                r.floor_gap,
                r.nosplit_lost,
                u8::from(r.split.is_some()),
                r.upgrade_leak,
                scores(&r.u1)
            ));
        }
        let path = std::env::var("NM_EV_TSV").unwrap_or_else(|_| "/tmp/sg-6j/ev.tsv".into());
        let _ = fs::write(&path, tsv);
        // (split lost above floor, leaked, split kept, split upgraded) of one record.
        let eval = |rule: &dyn Fn(&[f32]) -> bool, r: &EvRec| -> (usize, usize, usize, usize) {
            match r.split {
                None => (0, 0, 0, 0),
                Some((lost, leak)) if rule(&r.u1) => (lost, leak, 1, 0),
                Some(_) => (0, r.upgrade_leak, 0, 1),
            }
        };
        let mut md = String::from(
            "## U1 evidence for an other-to-caller split (chunk 6j, decision aa)\n\n### Floor and drops per set (no rule changes these)\n\n| set | utterances | reference words | STT floor (words measure A misses with no gate) | whole-utterance drops (window 1 other, no confirm): utterances / words above floor | other-to-caller splits today |\n| --- | --- | --- | --- | --- | --- |\n",
        );
        for (s, _) in &sets {
            let rs: Vec<&EvRec> = recs.iter().filter(|r| r.set == *s).collect();
            md.push_str(&format!(
                "| {s} | {} | {} | {} | {} / {} | {} |\n",
                rs.len(),
                rs.iter().map(|r| r.ref_words).sum::<usize>(),
                rs.iter().map(|r| r.floor_gap).sum::<usize>(),
                rs.iter().filter(|r| r.nosplit_lost > 0).count(),
                rs.iter().map(|r| r.nosplit_lost).sum::<usize>(),
                rs.iter().filter(|r| r.split.is_some()).count()
            ));
        }
        md.push_str("\n### Per set (split lost above floor / leaked words / splits kept / splits upgraded)\n\n| rule |");
        for (s, _) in &sets {
            md.push_str(&format!(" {s} |"));
        }
        md.push_str("\n| --- |");
        for _ in &sets {
            md.push_str(" --- |");
        }
        md.push('\n');
        let mut totals = Vec::new();
        for (name, rule) in &rules {
            md.push_str(&format!("| {name} |"));
            let mut t = (0usize, 0usize, 0usize, 0usize, 0usize);
            for (s, co) in &sets {
                let (mut l, mut k, mut sk, mut su) = (0, 0, 0, 0);
                for r in recs.iter().filter(|r| r.set == *s) {
                    let (a, b, c, d) = eval(rule.as_ref(), r);
                    l += a;
                    k += b;
                    sk += c;
                    su += d;
                }
                md.push_str(&format!(" {l} / {k} / {sk} / {su} |"));
                if *co {
                    t.0 += l;
                    t.1 += sk;
                } else {
                    t.2 += l;
                    t.3 += su;
                }
                t.4 += k;
            }
            md.push('\n');
            totals.push((name.clone(), t));
        }
        totals.sort_by_key(|(_, t)| (t.0 + t.2, t.4));
        md.push_str("\n### Ranked (split-dependent lost words above the floor, both sets; then leaked words)\n\n| rank | rule | caller-only lost above floor | caller-only false splits left | bystander->caller lost above floor | true splits upgraded | leaked words | total lost above floor |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for (i, (name, t)) in totals.iter().enumerate() {
            md.push_str(&format!(
                "| {} | {name} | {} | {} | {} | {} | {} | {} |\n",
                i + 1,
                t.0,
                t.1,
                t.2,
                t.3,
                t.4,
                t.0 + t.2
            ));
        }
        for (name, rule) in &rules {
            if !name.starts_with("R5")
                && (!(name.contains("< 0.25") || name.starts_with("R1:")) || name.contains(">="))
            {
                continue;
            }
            md.push_str(&format!(
                "\n### Caller-only false splits left under \"{name}\" (set, U1 scores, caller words lost above floor)\n\n"
            ));
            for r in recs.iter().filter(|r| r.caller_only) {
                if let Some((lost, _)) = r.split.filter(|_| rule(&r.u1)) {
                    md.push_str(&format!(
                        "- {}: U1 [{}], lost {lost}\n",
                        r.set,
                        scores(&r.u1)
                    ));
                }
            }
        }
        md.push_str("\nGate false accepts are unchanged by every rule: a rule only decides split or upgrade after the gate already confirmed `caller`. An upgrade counts no lost words above the floor; its leak is every bystander word of the merged utterance.\n\n");
        md.push_str(&veto_table(&recs, &sets));
        md
    }

    pub fn piper(embedder: &Embedder, dirs: &[PathBuf], threshold: f32) -> Result<String, String> {
        // (caller clip, reference, scene id) and bystander clips.
        let mut callers: Vec<(Clip, Vec<f32>, String)> = Vec::new();
        let mut bys: Vec<(Clip, String)> = Vec::new();
        for dir in dirs {
            let manifest = parse_json(
                &fs::read_to_string(dir.join("manifest.json"))
                    .map_err(|e| format!("manifest: {e}"))?,
            )?;
            let Some(Json::Arr(list)) = manifest.get("scenes") else {
                return Err("manifest has no scenes".into());
            };
            for scene in list {
                let id = text(scene, "id").unwrap_or_default();
                let Some(Json::Arr(segs)) = scene.get("segments") else {
                    continue;
                };
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
                let Some(t) = target_t(c.off, i * 3 + r) else {
                    continue;
                };
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
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                Some(
                    f.as_chunks::<3>()
                        .0
                        .iter()
                        .map(|c| (c[0] + c[1] + c[2]) / 3.0)
                        .collect(),
                )
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
                    let Some((bon, boff)) = voiced(&ba) else {
                        continue;
                    };
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
                            let utt = build_utt(&s, u, ue, embedder, &reference);
                            if EVIDENCE.get().is_some() {
                                far_o2c(cname, &s, &utt, (u, ue), embedder, &reference, threshold);
                            }
                            far.push(utt);
                        }
                    }
                }
                eprintln!("librispeech {cname} caller {ci} done");
            }
            md.push_str(&recovery_table(
                &format!("LibriSpeech straddle, {cname}"),
                &res,
            ));
            md.push_str(&far_table(
                &format!("LibriSpeech bystander-only, {cname}"),
                &far,
                threshold,
            ));
        }
        Ok(md)
    }

    /// The caller -> other classifier's blind spot (decision pp): window n-1 holds the last
    /// caller words, window n lies mostly past the voiced end, both `other`. The cut lands at
    /// most at window n-1's middle, so the old cut-point classifier says (c) mid-speech; the
    /// confirming window says (a) bed tail.
    #[cfg(test)]
    mod classifier_tests {
        use super::*;
        use crate::cand::{c3_step, parse_cand, C3Step};

        /// The classifier before decision pp: by the cut point.
        fn c2o_class_by_cut(trunc: bool, v_end: usize, cut: usize) -> usize {
            if trunc && cut + WINDOW_MS >= LEAD_MS + 15_000 {
                1
            } else if cut >= v_end {
                0
            } else {
                2
            }
        }

        #[test]
        fn confirming_window_past_voiced_end_is_bed_tail_not_mid_speech() {
            // 800 ms lead, caller tone 800..5000 ms, 2 s silent tail.
            let speech_end = 5000;
            let mut clean = vec![0f32; LEAD_MS * SAMPLES_PER_MS];
            clean.extend(
                (0..(speech_end - LEAD_MS) * SAMPLES_PER_MS).map(|i| {
                    0.3 * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 16_000.0).sin()
                }),
            );
            clean.extend(vec![0f32; TAIL_MS * SAMPLES_PER_MS]);
            let v_end = voiced(&clean).map(|v| v.1).unwrap();
            assert_eq!(v_end, speech_end);
            // Windows on the scene clock: caller, caller, other (holds the last caller words,
            // 3800..5300), other (5300..6800, all past v_end).
            let utt = Utt {
                wins: vec![
                    (800, 0.6, 0.0),
                    (2300, 0.6, 0.0),
                    (3800, 0.1, 0.0),
                    (5300, 0.1, 0.0),
                ],
                rechecks: Vec::new(),
            };
            let (cut, span) =
                ul_c2o_whole(&clean, &utt, 0.459, None).expect("two other windows confirm");
            assert_eq!(span, (5300, 6800));
            assert!(
                cut <= 3800 + WINDOW_MS / 2,
                "cut {cut} past window n-1's middle"
            );
            let old = c2o_class_by_cut(false, v_end, cut);
            let new = c2o_class(false, v_end, span);
            eprintln!("blind-spot check: cut {cut} ms, confirm {span:?}, v_end {v_end}: old class {old}, new class {new}");
            assert_eq!(old, 2, "old classifier: (c) mid-speech");
            assert_eq!(new, 0, "new classifier: (a) bed tail");
        }

        fn drive(mins: &[f32], min: f32, exhausted_at_end: bool) -> (usize, C3Step) {
            let mut best: Option<f32> = None;
            for (i, mn) in mins.iter().enumerate() {
                let tries = i + 1;
                let last = i + 1 == mins.len();
                let step = c3_step(*mn, min, tries, best, last && exhausted_at_end);
                if step != C3Step::Retry {
                    return (tries, step);
                }
                best = Some(best.map_or(*mn, |b| b.max(*mn)));
            }
            (mins.len(), C3Step::Retry)
        }

        #[test]
        fn c3_takes_the_best_try_when_files_run_out() {
            assert_eq!(drive(&[0.2, 0.25], 0.35, true), (2, C3Step::TakeBest(0.25)));
        }

        #[test]
        fn c3_takes_the_best_try_after_three_tries() {
            assert_eq!(
                drive(&[0.2, 0.25, 0.22], 0.35, false),
                (3, C3Step::TakeBest(0.25))
            );
        }

        #[test]
        fn c3_accepts_a_good_enrollment() {
            assert_eq!(drive(&[0.4], 0.35, false), (1, C3Step::Accept));
            assert_eq!(drive(&[0.2, 0.4], 0.35, false), (2, C3Step::Accept));
        }

        #[test]
        fn parse_cand_rejects_unknown_tokens() {
            assert!(parse_cand("c4", 0.35).is_err());
            assert!(parse_cand("C2", 0.35).is_err());
            assert!(parse_cand("c7r", 0.35).is_err());
            assert!(parse_cand("c4n0", 0.35).is_err());
            assert!(parse_cand("c7r1", 0.35).is_err());
            assert!(parse_cand("c2,bogus", 0.35).is_err());
        }

        #[test]
        fn parse_cand_defaults_and_fields() {
            let d = parse_cand("", 0.35).unwrap();
            assert_eq!(d.enroll_ms, 3000);
            assert_eq!(d.c7_run, 2);
            assert!(!(d.c2 || d.c3 || d.c7_nm || d.c7_scan));
            assert_eq!(d.c4, 0);
            let c = parse_cand("e5000,c2,c4n2,c7r3,c7nm", 0.4).unwrap();
            assert_eq!((c.enroll_ms, c.c4, c.c7_run), (5000, 2, 3));
            assert!(c.c2 && c.c7_nm && !c.c3 && !c.c7_scan);
            assert_eq!(c.c3_min, 0.4);
            assert!(parse_cand("c7scan,c3", 0.35).unwrap().c7_scan);
        }

        #[test]
        fn c4_turns_a_dropped_caller_only_utterance_into_caller() {
            let w = |v: &[f32]| -> Vec<Win> { v.iter().map(|s| (0, *s, 0.0)).collect() };
            let t = 0.459;
            // Window 1 `other`, no confirm: today the utterance is dropped (7 words lost).
            let near = w(&[0.31, 0.35, 0.33]);
            assert_eq!(no_confirm(&near, t, 0, false, 7, true), (false, 7));
            // C4 n=3: accepted whole, no caller loss.
            assert_eq!(no_confirm(&near, t, 3, false, 7, true), (true, 0));
            // A window below 0.20: not accepted, still dropped.
            let low = w(&[0.31, 0.35, 0.33, 0.19]);
            assert_eq!(no_confirm(&low, t, 3, false, 7, true), (false, 7));
            // Fewer than N near-miss windows: not accepted.
            assert_eq!(
                no_confirm(&w(&[0.31, 0.35]), t, 3, false, 7, true),
                (false, 7)
            );
            // The deferred veto already ended `caller`: nothing lost, not a C4 accept.
            assert_eq!(no_confirm(&near, t, 3, true, 7, true), (false, 0));
        }

        #[test]
        fn c4_accept_band() {
            let t = 0.42;
            assert!(c4_scores(&[0.31, 0.35, 0.25], t, 2));
            assert!(!c4_scores(&[0.31, 0.35, 0.19], t, 2));
            assert!(!c4_scores(&[0.31, 0.25, 0.25], t, 2));
            assert!(!c4_scores(&[0.42, 0.31], t, 2));
            assert!(!c4_scores(&[0.31, 0.35, 0.25], t, 0));
        }

        fn win(i: u32, score: f32) -> U2Win {
            ((i * 1500, i * 1500 + 1500), vec![i as u8; 4], score)
        }

        fn c7(run: usize, nm: bool, scan: bool) -> C7 {
            C7 { run, nm, scan }
        }

        fn cut(wins: &[U2Win], c: C7, hit: u8) -> Option<(u32, u32)> {
            let mut scan = |clip: &[u8]| clip.first() == Some(&hit);
            u2_cut_core(wins, 0, 0.459, 0, c, &mut scan).map(|(_, span)| span)
        }

        #[test]
        fn c7_near_miss_window_is_neutral() {
            let w = [win(0, 0.1), win(1, 0.35), win(2, 0.1)];
            // Neutral: [other, near-miss, other] with run 2 confirms on the third window.
            assert_eq!(cut(&w, c7(2, true, false), 99), Some((3000, 4500)));
            // Today's rules: the near-miss window counts as `other` and confirms on the second.
            assert_eq!(cut(&w, c7(2, false, false), 99), Some((1500, 3000)));
            // A near-miss window does not reset: [other, nm, nm] never confirms.
            assert_eq!(
                cut(
                    &[win(0, 0.1), win(1, 0.35), win(2, 0.35)],
                    c7(2, true, false),
                    99
                ),
                None
            );
        }

        #[test]
        fn c7_run_of_three() {
            let w = [win(0, 0.1), win(1, 0.1), win(2, 0.1)];
            assert_eq!(cut(&w, c7(3, false, false), 99), Some((3000, 4500)));
            assert_eq!(cut(&w[..2], c7(3, false, false), 99), None);
        }

        #[test]
        fn c7_scan_hit_vetoes_the_cut_and_restarts_the_run() {
            let w = [win(0, 0.1), win(1, 0.1), win(2, 0.1), win(3, 0.1)];
            // A sub-window hit in window 1's clip vetoes the cut at window 1; the run restarts at
            // window 2 and confirms at window 3 (window 1 is the new `prev`, so clean).
            assert_eq!(cut(&w, c7(2, false, true), 1), Some((4500, 6000)));
            // A hit in the first window of the run vetoes too.
            assert_eq!(cut(&w[..2], c7(2, false, true), 0), None);
            // No hit: the cut is confirmed as without the scan.
            assert_eq!(cut(&w[..2], c7(2, false, true), 99), Some((1500, 3000)));
            // The scan is off: a would-be hit does nothing.
            assert_eq!(cut(&w[..2], c7(2, false, false), 0), Some((1500, 3000)));
        }

        #[test]
        fn confirming_window_after_truncation_is_artefact() {
            let ce = LEAD_MS + 15_000 + 700;
            assert_eq!(c2o_class(true, LEAD_MS + 15_000, (ce - WINDOW_MS, ce)), 1);
            assert_eq!(c2o_class(false, LEAD_MS + 14_000, (ce - WINDOW_MS, ce)), 0);
            assert_eq!(c2o_class(true, 9000, (3000, 4500)), 2);
        }
    }
}
