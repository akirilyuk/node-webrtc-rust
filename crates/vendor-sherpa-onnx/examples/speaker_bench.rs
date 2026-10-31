//! Speaker-ID bench on the scenes of `speaker_fixtures`: how well does a speaker-embedding model
//! tell the main voice from quieter background voices, and what does that do to what the voice
//! agent would transcribe and attribute?
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_bench -- \
//!   --fixtures <speaker_fixtures out dir> --models <speaker dir> --stt-model <zipformer dir> \
//!   --out <dir> [--vad-min-silence-ms <n>] \
//!   [--enhance <denoiser models dir> [--pre raw,rnnoise,gtcrn,...] [--cost-seconds <n>] [--streams-seconds <n>]]
//!   [--checks 0.5,1.0,1.5] [--fixed-threshold <t> [--recheck]]
//!
//! `--checks` replaces the Part 1 check lengths (seconds). `--fixed-threshold` runs Part 1 only and
//! also scores every check length at that one threshold (FRR/FAR per set and level), written to
//! `<out>/part1_fixed.md`; Parts 2 and 3 are skipped. `--recheck` adds, per check length, the
//! FAR of the speaker gate's re-check (`recheckOnOther`) on impostor segments long enough for two
//! windows: accepted when the first window scores at or above the threshold, or when it scores
//! below and the second window `[check, 2 x check)` does (one row per set, levels pooled).
//! ```
//!
//! Every Sherpa object is single-threaded (`num_threads: 1`); pin the process to one core
//! (`taskset -c 2`) so the CPU numbers map onto a 1000m pod.
//!
//! **Part 1, embedding level** (per embedding model x check length 0.5 / 1.0 / 1.5 s x level):
//! - reference = embedding of the two enrollment utterances of the main voice in the S3 scene of
//!   the item's set and level, concatenated (fallback: another S3 scene of the set, then any);
//! - genuine = the first `check` seconds of every S1 main phrase and every S3 main turn;
//! - impostor = the first `check` seconds of every S2 bystander utterance and every S3 bystander
//!   turn;
//! - rows `-6`, `-10`, `-15`, `-20` pool the items of that level (S2 items, which have no level,
//!   belong to every row), row `all` pools everything, row `real` is the Sherpa real-voice clips
//!   (enroll `*-sr-*`, test `*-test-sr-*`; clean speech, no tune/test split);
//! - per row: EER, FRR at FAR 1 %, and the threshold at FAR 1 %, all from the **tune** set only;
//!   then that threshold applied to the **test** set (FRR/FAR there). Scores are cosine
//!   similarities of two embeddings; an utterance is accepted when its score is above the threshold.
//! - `compute` time (stream, waveform, embedding) p50/p99 per model and check length.
//!
//! **Part 2, end to end** (the voice agent's path): every S3 scene of the test set (of the tune
//! set when the fixtures have no test-set S3, marked in the summary) is fed in 20 ms frames of
//! 48 kHz stereo PCM through the speech crate's `VadEngine` with `VadConfig::default()` (the energy
//! VAD; Silero was evaluated and dropped, see benchmarks/speaker/README.md). Only
//! `min_silence_duration_ms` may be overridden, with `--vad-min-silence-ms`. For each VAD utterance:
//! - the Zipformer online recognizer transcribes the VAD window (the product STT pre-roll,
//!   `speech_pad_ms + min_speech_duration_ms`, before the SpeechStart frame, + speech + trailing
//!   silence); qualified phrases are scored with the roundtrip oracle (`wordSimilarity`), long
//!   sentences are only measured (WER, `wordSimilarity`; they are not qualified);
//! - the first `check` seconds from the start of that window are embedded (model and check length
//!   with the lowest tune-set EER in row `all`) and compared with the scene's enrollment using
//!   the tune-set threshold: caller or other;
//! - the utterance is matched to the manifest segment (main or bystander) it overlaps most, if
//!   that overlap is at least 50 % of the utterance; otherwise it matches no segment.
//!
//! Per level it reports: (a) share of main turns whose transcript passes the oracle
//! (`wordSimilarity` >= 0.75), (b) share of main turns attributed to the caller, (c) share of
//! bystander turns attributed to others, (d) share of bystander turns attributed to the caller
//! (false accepts), (e) background-word leakage (words of the background sentences, 4+ letters and
//! not in a main phrase, found in main-turn transcripts, divided by the words of those
//! transcripts), (f) VAD utterances that match no segment and how they were attributed. Turns
//! that no utterance matches are counted as missed.
//!
//! Output: `<out>/results.json` (all numbers plus `nproc`, CPU model, git sha from
//! `NWR_BENCH_GIT_SHA` or `git rev-parse HEAD`, manifest sha256), `<out>/summary.md`, and the
//! per-turn record: `<out>/turns.jsonl` (one row per manifest segment of every S3 scene and VAD
//! variant: who spoke, what was said, what was expected, which VAD utterances overlapped it, the
//! transcript, the speaker score and verdict, the result; plus one row per VAD utterance that
//! matches no segment, and `merged` ids when an utterance covers two or more turns) and
//! `<out>/conversations.md` (the same as a timeline table per scene).
//!
//! **Part 3, enhancement** (`--enhance <dir>`): the Part 2 pipeline with a preprocessor in front
//! of the VAD / STT split (`support/enhance.rs`): `pre` in raw, rnnoise, gtcrn, dpdfnet_baseline,
//! dpdfnet2, dpdfnet4, dpdfnet8, dpdfnet2_48khz_hr (`--pre` restricts the list), each with the
//! caller AGC off and on. Grid A: all combos over the S3 scenes at main -20 dBFS; Grid B: caller
//! level (main -26 / -20 / -14 dBFS at relative levels -15 and -10) for raw, rnnoise, gtcrn and
//! dpdfnet2. Cost per preprocessor (RTF p50 / p99 per 20 ms frame, streams per core, added
//! delay) and the S1 whole-clip WER per level (including clean) are reported too.
//! `--pre ...,precomputed:<name>=<dir>` adds a preprocessor whose output was computed elsewhere
//! (VoxLock, `benchmarks/speaker/voxlock_preprocess.py`): scene audio is read from
//! `<dir>/scenes/<id>.wav`, the cost row from `<dir>/voxlock_cost.json`. Measurement only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

#[path = "support/mod.rs"]
mod support;

#[path = "support/enhance.rs"]
mod enhance;

use enhance::{process_pcm, Agc, Preprocessor, SherpaKind};

use support::{
    create_recognizer, json_escape, normalize_for_compare, parse_json, read_wav,
    read_wav_stereo_48k_pcm, recognize, sha256_hex, word_similarity, Json,
};

const SR: i32 = 16_000;
const CHECKS_S: [f32; 3] = [0.5, 1.0, 1.5];
const MAX_CHECK_SAMPLES: usize = 24_000;
const FAR_TARGET: f64 = 0.01;
const ORACLE_MIN: f64 = 0.75;
const TAIL_SILENCE_MS: usize = 2500;
const LEVEL_ROWS: [&str; 5] = ["-6", "-10", "-15", "-20", "all"];

// ---------------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Set {
    Tune,
    Test,
}

#[derive(Clone)]
struct Seg {
    start_ms: usize,
    end_ms: usize,
    role: String,
    voice: String,
    sid: i32,
    text: String,
    enroll: bool,
    /// `qualified` (swept, may be judged), `long` (measured only) or `background`.
    kind: String,
    /// `caller` or `other`: who the agent should attribute the segment to.
    expected_speaker: String,
    /// What should be heard; `None` for babble.
    expected_transcript: Option<String>,
}

struct SceneInfo {
    id: String,
    kind: String,
    set: Set,
    level_db: i32,
    /// Main voice level in dBFS; -20 unless the fixtures were built with `--main-levels`.
    main_db: i32,
    /// Voiced RMS of the main voice after the fixtures' peak limiter, dBFS (`None` without main
    /// speech or in manifests that predate the limiter).
    effective_main_db: Option<f64>,
    segments: Vec<Seg>,
}

fn str_of(json: &Json, key: &str) -> Result<String, String> {
    match json.get(key) {
        Some(Json::Str(s)) => Ok(s.clone()),
        _ => Err(format!("manifest: missing string \"{key}\"")),
    }
}

fn num_of(json: &Json, key: &str) -> Result<f64, String> {
    match json.get(key) {
        Some(Json::Num(n)) => Ok(*n),
        _ => Err(format!("manifest: missing number \"{key}\"")),
    }
}

fn load_manifest(text: &str) -> Result<Vec<SceneInfo>, String> {
    let root = parse_json(text)?;
    let Some(Json::Arr(scenes)) = root.get("scenes") else {
        return Err("manifest: no scenes".into());
    };
    let mut out = Vec::new();
    for scene in scenes {
        let set = match str_of(scene, "set")?.as_str() {
            "tune" => Set::Tune,
            "test" => Set::Test,
            other => return Err(format!("manifest: bad set {other}")),
        };
        let Some(Json::Arr(segs)) = scene.get("segments") else {
            return Err("manifest: scene without segments".into());
        };
        let mut segments = Vec::new();
        for seg in segs {
            let expected = seg
                .get("expected")
                .ok_or("manifest: segment without \"expected\"")?;
            let expected_transcript = match expected.get("transcript") {
                Some(Json::Str(t)) => Some(t.clone()),
                Some(Json::Null) => None,
                _ => return Err("manifest: expected.transcript must be a string or null".into()),
            };
            segments.push(Seg {
                start_ms: num_of(seg, "start_ms")? as usize,
                end_ms: num_of(seg, "end_ms")? as usize,
                role: str_of(seg, "role")?,
                voice: str_of(seg, "voice")?,
                sid: num_of(seg, "sid")? as i32,
                text: str_of(seg, "text")?,
                enroll: matches!(seg.get("enroll"), Some(Json::Bool(true))),
                kind: str_of(seg, "kind")?,
                expected_speaker: str_of(expected, "speaker")?,
                expected_transcript,
            });
        }
        out.push(SceneInfo {
            id: str_of(scene, "id")?,
            kind: str_of(scene, "kind")?,
            set,
            level_db: num_of(scene, "level_db")? as i32,
            main_db: match scene.get("main_db") {
                Some(Json::Num(n)) => *n as i32,
                _ => -20,
            },
            effective_main_db: match scene.get("effective_main_db") {
                Some(Json::Num(n)) => Some(*n),
                _ => None,
            },
            segments,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------------------------

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

/// (FAR, FRR) at `threshold`; an utterance is accepted when its score is above the threshold.
fn far_frr(genuine: &[f32], impostor: &[f32], threshold: f32) -> (Option<f64>, Option<f64>) {
    let far = (!impostor.is_empty()).then(|| {
        impostor.iter().filter(|s| **s > threshold).count() as f64 / impostor.len() as f64
    });
    let frr = (!genuine.is_empty())
        .then(|| genuine.iter().filter(|s| **s <= threshold).count() as f64 / genuine.len() as f64);
    (far, frr)
}

/// Equal error rate and the threshold where it is reached.
fn eer(genuine: &[f32], impostor: &[f32]) -> Option<(f64, f32)> {
    if genuine.is_empty() || impostor.is_empty() {
        return None;
    }
    let mut candidates: Vec<f32> = genuine.iter().chain(impostor).copied().collect();
    candidates.sort_by(|a, b| a.total_cmp(b));
    candidates.dedup();
    let mut best: Option<(f64, f64, f32)> = None; // (|far-frr|, eer, threshold)
    for t in candidates {
        let (far, frr) = far_frr(genuine, impostor, t);
        let (far, frr) = (far?, frr?);
        let gap = (far - frr).abs();
        if best.is_none_or(|(g, _, _)| gap < g) {
            best = Some((gap, (far + frr) / 2.0, t));
        }
    }
    best.map(|(_, e, t)| (e, t))
}

/// Smallest threshold whose FAR on `impostor` is at most `far` (accept = score above it).
fn threshold_at_far(impostor: &[f32], far: f64) -> Option<f32> {
    if impostor.is_empty() {
        return None;
    }
    let mut sorted = impostor.to_vec();
    sorted.sort_by(|a, b| b.total_cmp(a)); // descending
    let allowed = (far * sorted.len() as f64).floor() as usize;
    Some(if allowed < sorted.len() {
        sorted[allowed]
    } else {
        f32::NEG_INFINITY
    })
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn pct(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| 100.0 * n as f64 / d as f64)
}

fn fmt_opt(v: Option<f64>, digits: usize) -> String {
    v.map_or("n/a".to_string(), |x| format!("{x:.digits$}"))
}

fn json_num(v: Option<f64>) -> String {
    v.filter(|x| x.is_finite())
        .map_or("null".to_string(), |x| format!("{x:.6}"))
}

// ---------------------------------------------------------------------------------------------
// Embedding
// ---------------------------------------------------------------------------------------------

struct Embedder {
    id: String,
    extractor: SpeakerEmbeddingExtractor,
    /// seconds per `compute`, per check length index (None for references and real-voice clips).
    timings: Vec<Vec<f64>>,
}

impl Embedder {
    fn open(path: &Path, n_checks: usize) -> Result<Self, String> {
        let config = SpeakerEmbeddingExtractorConfig {
            model: Some(path.to_string_lossy().into_owned()),
            num_threads: 1,
            ..Default::default()
        };
        let extractor = SpeakerEmbeddingExtractor::create(&config)
            .ok_or_else(|| format!("cannot open speaker model {}", path.display()))?;
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();
        Ok(Self {
            id,
            extractor,
            timings: vec![Vec::new(); n_checks],
        })
    }

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

fn check_samples(check_s: f32) -> usize {
    (check_s * SR as f32) as usize
}

fn first(samples: &[f32], len: usize) -> &[f32] {
    &samples[..samples.len().min(len)]
}

// ---------------------------------------------------------------------------------------------
// Part 1
// ---------------------------------------------------------------------------------------------

struct Item {
    set: Set,
    /// Level of the scene (S1, S3); None for S2.
    level: Option<i32>,
    genuine: bool,
    ref_scene: usize,
    /// Up to 3 s from the start of the segment (two 1.5 s windows for `--recheck`).
    clip: Vec<f32>,
}

struct Part1Row {
    model: String,
    check_s: f32,
    level: String,
    n_gen_tune: usize,
    n_imp_tune: usize,
    eer_tune: Option<f64>,
    frr_at_far1_tune: Option<f64>,
    threshold_tune: Option<f32>,
    n_gen_test: usize,
    n_imp_test: usize,
    frr_test: Option<f64>,
    far_test: Option<f64>,
}

struct Fixtures {
    dir: PathBuf,
    scenes: Vec<SceneInfo>,
    audio: HashMap<usize, Vec<f32>>,
}

impl Fixtures {
    fn audio16(&mut self, scene: usize) -> Result<&Vec<f32>, String> {
        if !self.audio.contains_key(&scene) {
            let path = self
                .dir
                .join("scenes")
                .join(format!("{}.16k.wav", self.scenes[scene].id));
            let samples = read_wav(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            self.audio.insert(scene, samples);
        }
        Ok(&self.audio[&scene])
    }

    /// The S3 scene whose enrollment is the reference for `(set, level)`.
    fn reference_scene(&self, set: Set, level: Option<i32>) -> Option<usize> {
        let s3 =
            |i: &usize| self.scenes[*i].kind == "conversation" && self.scenes[*i].main_db == -20;
        let candidates: Vec<usize> = (0..self.scenes.len()).filter(s3).collect();
        let same_set: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|i| self.scenes[*i].set == set)
            .collect();
        if let Some(l) = level {
            if let Some(i) = same_set.iter().find(|i| self.scenes[**i].level_db == l) {
                return Some(*i);
            }
        } else if let Some(i) = same_set.iter().find(|i| self.scenes[**i].level_db == -10) {
            return Some(*i);
        }
        same_set.first().or(candidates.first()).copied()
    }
}

/// A genuine trial with the text of an enrollment utterance is the same waveform as the
/// enrollment (TTS clip cache), so it is not a trial.
fn shares_enrollment_text(text: &str, enroll_texts: &[String]) -> bool {
    enroll_texts.iter().any(|e| e == text)
}

/// Sample range `[start, min(start + check, end))` of the Part 2 embedding clip, in samples at
/// 16 kHz from millisecond bounds: never past the end the VAD reported for the utterance.
fn part2_clip_range(start_ms: usize, end_ms: usize, check_s: f32) -> (usize, usize) {
    let from = start_ms * 16;
    let to = (end_ms * 16).min(from + check_samples(check_s));
    (from, to.max(from))
}

/// Parts 1 and 2 use the default caller level only (-20 dB); the other caller-level conversation
/// scenes belong to Part 3.
fn default_level_scene(kind: &str, main_db: i32) -> bool {
    kind != "conversation" || main_db == -20
}

fn collect_items(fx: &mut Fixtures) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    for scene in 0..fx.scenes.len() {
        let kind = fx.scenes[scene].kind.clone();
        if !default_level_scene(&kind, fx.scenes[scene].main_db) {
            continue;
        }
        let set = fx.scenes[scene].set;
        let scene_level = fx.scenes[scene].level_db;
        let level = (kind != "bystander_alone").then_some(scene_level);
        let ref_scene = fx
            .reference_scene(set, level)
            .ok_or("fixtures have no conversation scene (needed for the enrollment reference)")?;
        let enroll_texts: Vec<String> = fx.scenes[ref_scene]
            .segments
            .iter()
            .filter(|s| s.enroll)
            .map(|s| s.text.clone())
            .collect();
        let segs: Vec<(usize, usize, String, bool, String)> = fx.scenes[scene]
            .segments
            .iter()
            .map(|s| {
                (
                    s.start_ms,
                    s.end_ms,
                    s.role.clone(),
                    s.enroll,
                    s.text.clone(),
                )
            })
            .collect();
        for (start_ms, end_ms, role, enroll, text) in segs {
            let genuine = match (kind.as_str(), role.as_str()) {
                ("main_over_babble", "main") => true,
                ("conversation", "main") if !enroll => true,
                ("bystander_alone", "bystander") | ("conversation", "bystander") => false,
                _ => continue,
            };
            if genuine && shares_enrollment_text(&text, &enroll_texts) {
                continue;
            }
            let audio = fx.audio16(scene)?;
            let from = (start_ms * 16).min(audio.len());
            let to = (end_ms * 16)
                .min(audio.len())
                .min(from + 2 * MAX_CHECK_SAMPLES);
            items.push(Item {
                set,
                level,
                genuine,
                ref_scene,
                clip: audio[from..to].to_vec(),
            });
        }
    }
    Ok(items)
}

/// Millisecond ranges of the enrollment segments of a scene.
fn enrollment_ranges(fx: &Fixtures, scene: usize) -> Result<Vec<(usize, usize)>, String> {
    let ranges: Vec<(usize, usize)> = fx.scenes[scene]
        .segments
        .iter()
        .filter(|s| s.enroll)
        .map(|s| (s.start_ms, s.end_ms))
        .collect();
    if ranges.is_empty() {
        return Err(format!(
            "scene {} has no enrollment segment",
            fx.scenes[scene].id
        ));
    }
    Ok(ranges)
}

/// The enrollment ranges cut out of 16 kHz `audio` and concatenated.
fn enrollment_clip(ranges: &[(usize, usize)], audio: &[f32]) -> Vec<f32> {
    let mut out = Vec::new();
    for (start, end) in ranges {
        out.extend_from_slice(&audio[(start * 16).min(audio.len())..(end * 16).min(audio.len())]);
    }
    out
}

/// Enrollment of a scene: its enrollment segments concatenated.
fn enrollment(fx: &mut Fixtures, scene: usize) -> Result<Vec<f32>, String> {
    let ranges = enrollment_ranges(fx, scene)?;
    let audio = fx.audio16(scene)?.clone();
    Ok(enrollment_clip(&ranges, &audio))
}

fn level_matches(row: &str, item: &Item) -> bool {
    row == "all"
        || item.level.is_none()
        || item.level.map(|l| l.to_string()).as_deref() == Some(row)
}

/// Part 1 at one fixed threshold: FRR (genuine rejected) and FAR (impostor accepted) per set and
/// level row, as a markdown table.
fn part1_fixed_table(
    model: &str,
    check_s: f32,
    threshold: f32,
    items: &[Item],
    scores: &[Option<f32>],
) -> String {
    let pct = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{:.1} %", v * 100.0));
    let mut md = format!(
        "### {model} / {check_s} s at threshold {threshold}\n\n| set | level | n gen | FRR | n imp | FAR |\n|---|---|---|---|---|---|\n"
    );
    for (set, name) in [(Set::Tune, "tune"), (Set::Test, "test")] {
        for row in LEVEL_ROWS {
            let pick = |genuine: bool| -> Vec<f32> {
                items
                    .iter()
                    .zip(scores)
                    .filter(|(it, _)| {
                        it.set == set && it.genuine == genuine && level_matches(row, it)
                    })
                    .filter_map(|(_, s)| *s)
                    .collect()
            };
            let (g, i) = (pick(true), pick(false));
            let (far, frr) = far_frr(&g, &i, threshold);
            md.push_str(&format!(
                "| {name} | {row} | {} | {} | {} | {} |\n",
                g.len(),
                pct(frr),
                i.len(),
                pct(far)
            ));
        }
    }
    md.push('\n');
    md
}

/// Re-check FAR at a fixed threshold, on impostor segments with two full windows: one-window FAR
/// (first window accepted) next to the two-window FAR (first or, after a reject, second window
/// accepted), per set with levels pooled. Accept = score >= threshold, as in the product.
fn recheck_far_table(
    model: &str,
    check_s: f32,
    threshold: f32,
    items: &[Item],
    first_scores: &[Option<f32>],
    second_scores: &[Option<f32>],
) -> String {
    let mut md = format!(
        "### re-check FAR: {model} / {check_s} s windows at threshold {threshold}\n\n| set | n imp (>= 2 windows) | FAR 1 window | FAR 2 windows |\n|---|---|---|---|\n"
    );
    for (set, name) in [(Set::Tune, "tune"), (Set::Test, "test")] {
        let (mut n, mut one, mut two) = (0_usize, 0_usize, 0_usize);
        for ((it, a), b) in items.iter().zip(first_scores).zip(second_scores) {
            let (Some(a), Some(b)) = (a, b) else {
                continue;
            };
            if it.set != set || it.genuine {
                continue;
            }
            n += 1;
            let first_ok = *a >= threshold;
            one += usize::from(first_ok);
            two += usize::from(first_ok || *b >= threshold);
        }
        let pct = |k: usize| {
            if n == 0 {
                "-".to_string()
            } else {
                format!("{:.1} % ({k}/{n})", k as f64 * 100.0 / n as f64)
            }
        };
        md.push_str(&format!("| {name} | {n} | {} | {} |\n", pct(one), pct(two)));
    }
    md.push('\n');
    md
}

fn part1_rows(model: &str, check_s: f32, items: &[Item], scores: &[Option<f32>]) -> Vec<Part1Row> {
    let mut rows = Vec::new();
    for row in LEVEL_ROWS {
        // S1/S3 genuine items only belong to their own level row; S2 impostors to every row.
        let pick = |set: Set, genuine: bool| -> Vec<f32> {
            items
                .iter()
                .zip(scores)
                .filter(|(it, _)| it.set == set && it.genuine == genuine && level_matches(row, it))
                .filter_map(|(_, s)| *s)
                .collect()
        };
        let (g_tune, i_tune) = (pick(Set::Tune, true), pick(Set::Tune, false));
        let (g_test, i_test) = (pick(Set::Test, true), pick(Set::Test, false));
        let eer_tune = eer(&g_tune, &i_tune);
        let threshold = threshold_at_far(&i_tune, FAR_TARGET);
        let frr_at_far1_tune = threshold.and_then(|t| far_frr(&g_tune, &i_tune, t).1);
        let (far_test, frr_test) = threshold.map_or((None, None), |t| far_frr(&g_test, &i_test, t));
        rows.push(Part1Row {
            model: model.to_string(),
            check_s,
            level: row.to_string(),
            n_gen_tune: g_tune.len(),
            n_imp_tune: i_tune.len(),
            eer_tune: eer_tune.map(|e| e.0),
            frr_at_far1_tune,
            threshold_tune: threshold,
            n_gen_test: g_test.len(),
            n_imp_test: i_test.len(),
            frr_test,
            far_test,
        });
    }
    rows
}

struct RealRow {
    model: String,
    check_s: f32,
    n_gen: usize,
    n_imp: usize,
    eer: Option<f64>,
    frr_at_far1: Option<f64>,
    threshold: Option<f32>,
}

fn real_voice_row(
    emb: &Embedder,
    speaker_dir: &Path,
    check_s: f32,
) -> Result<Option<RealRow>, String> {
    // name -> (enroll samples, test clips)
    let mut speakers: BTreeMap<String, (Vec<f32>, Vec<Vec<f32>>)> = BTreeMap::new();
    let mut names: Vec<String> = fs::read_dir(speaker_dir)
        .map_err(|e| format!("read {}: {e}", speaker_dir.display()))?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".wav"))
        .collect();
    names.sort();
    for name in names {
        let who = name.split('-').next().unwrap_or("").to_string();
        let samples = read_wav(&speaker_dir.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        let entry = speakers.entry(who).or_default();
        if name.contains("-test-") {
            entry.1.push(samples);
        } else {
            entry.0.extend_from_slice(&samples);
        }
    }
    let refs: BTreeMap<String, Vec<f32>> = speakers
        .iter()
        .filter(|(_, (enroll, _))| !enroll.is_empty())
        .filter_map(|(who, (enroll, _))| emb.embed(enroll).map(|(e, _)| (who.clone(), e)))
        .collect();
    let (mut genuine, mut impostor) = (Vec::new(), Vec::new());
    for (who, (_, tests)) in &speakers {
        for clip in tests {
            let Some((e, _)) = emb.embed(first(clip, check_samples(check_s))) else {
                continue;
            };
            for (ref_who, reference) in &refs {
                let score = cosine(reference, &e);
                if ref_who == who {
                    genuine.push(score);
                } else {
                    impostor.push(score);
                }
            }
        }
    }
    if genuine.is_empty() || impostor.is_empty() {
        return Ok(None);
    }
    let threshold = threshold_at_far(&impostor, FAR_TARGET);
    Ok(Some(RealRow {
        model: emb.id.clone(),
        check_s,
        n_gen: genuine.len(),
        n_imp: impostor.len(),
        eer: eer(&genuine, &impostor).map(|e| e.0),
        frr_at_far1: threshold.and_then(|t| far_frr(&genuine, &impostor, t).1),
        threshold,
    }))
}

// ---------------------------------------------------------------------------------------------
// Part 2
// ---------------------------------------------------------------------------------------------

#[derive(Default, Clone)]
struct LevelStats {
    main_turns: usize,
    main_missed: usize,
    main_caller: usize,
    /// Qualified phrases: judged with the oracle.
    q_turns: usize,
    q_recognized: usize,
    q_caller: usize,
    /// Long sentences: measured only (WER, word similarity), never judged.
    l_turns: usize,
    l_caller: usize,
    l_edits: usize,
    l_ref_words: usize,
    l_sim_sum: f64,
    byst_turns: usize,
    byst_missed: usize,
    byst_other: usize,
    byst_caller: usize,
    leak_words: usize,
    main_words: usize,
    unmatched: usize,
    unmatched_caller: usize,
    unmatched_other: usize,
    utterances: usize,
    /// VAD utterances that cover two or more non-babble segments.
    merged: usize,
    /// Per detected main turn: utterance start minus segment start, in ms.
    onset_delays: Vec<i64>,
    /// Preprocessor calls whose output had to be padded with silence.
    underruns: usize,
}

struct Utt {
    /// Start of speech (SpeechStart minus the minimum speech duration).
    start_ms: usize,
    /// The frame where the VAD declared SpeechStart.
    speech_start_ms: usize,
    /// End of the last active frame.
    end_active_ms: usize,
    /// SpeechEnd frame (includes the trailing silence).
    window_end_ms: usize,
}

fn run_vad(pcm: &[u8], config: &VadConfig) -> Result<Vec<Utt>, String> {
    let mut vad = VadEngine::new(config.clone()).map_err(|e| e.to_string())?;
    let frame_bytes = 960 * 2 * 2; // 20 ms of 48 kHz stereo s16le
    let mut utterances = Vec::new();
    let mut start: Option<(usize, usize)> = None; // (start_ms, speech_start_ms)
    let mut last_active_end = 0usize;
    let mut total_ms = 0usize;
    for (i, frame) in pcm.chunks_exact(frame_bytes).enumerate() {
        let t = i * 20;
        total_ms = t + 20;
        let (transitions, active) = vad
            .process_webrtc_pcm(frame, 20)
            .map_err(|e| e.to_string())?;
        if active {
            last_active_end = t + 20;
        }
        for transition in transitions {
            match transition {
                VadTransition::SpeechStart => {
                    start = Some((
                        (t + 20).saturating_sub(config.min_speech_duration_ms as usize),
                        t + 20,
                    ));
                }
                VadTransition::SpeechEnd => {
                    if let Some((s, speech_start)) = start.take() {
                        utterances.push(Utt {
                            start_ms: s,
                            speech_start_ms: speech_start,
                            end_active_ms: last_active_end.max(s),
                            window_end_ms: t + 20,
                        });
                    }
                }
            }
        }
    }
    if let Some((s, speech_start)) = start {
        utterances.push(Utt {
            start_ms: s,
            speech_start_ms: speech_start,
            end_active_ms: last_active_end.max(s),
            window_end_ms: total_ms,
        });
    }
    Ok(utterances)
}

fn overlap_ms(a: (usize, usize), b: (usize, usize)) -> usize {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

/// The pre-roll the product STT feed prepends at VAD `SpeechStart`
/// (`SttPreRollBuffer::from_vad_config`: `speech_pad_ms + min_speech_duration_ms`).
fn pre_roll_ms(config: &VadConfig) -> u32 {
    node_webrtc_rust_speech::stt_pre_roll::stt_pre_roll_capacity_ms(config)
}

/// Ids of the non-babble segments an utterance covers (at least half of the segment lies inside
/// the utterance span), when it covers two or more: such an utterance merged two turns.
/// `segments` are `(start_ms, end_ms, is_babble)`, the id is the index.
fn merged_segments(span: (usize, usize), segments: &[(usize, usize, bool)]) -> Vec<usize> {
    let covered: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter(|(_, (s, e, babble))| {
            !babble && overlap_ms(span, (*s, *e)) * 2 >= e.saturating_sub(*s).max(1)
        })
        .map(|(i, _)| i)
        .collect();
    if covered.len() >= 2 {
        covered
    } else {
        Vec::new()
    }
}

/// Word-level Levenshtein distance between the normalized texts, and the reference word count.
fn word_edits(reference: &str, hypothesis: &str) -> (usize, usize) {
    let r = words_of(reference);
    let h = words_of(hypothesis);
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut cur = vec![i; h.len() + 1];
        for j in 1..=h.len() {
            let sub = prev[j - 1] + usize::from(r[i - 1] != h[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    (prev[h.len()], r.len())
}

/// Word error rate: edits divided by the reference words.
#[cfg(test)]
fn wer(reference: &str, hypothesis: &str) -> f64 {
    let (edits, n) = word_edits(reference, hypothesis);
    if n == 0 {
        0.0
    } else {
        edits as f64 / n as f64
    }
}

/// The non-babble segment `(index, overlap_ms)` an utterance span matches: the one it overlaps
/// most, if that overlap is at least half of the span. `segments` are `(start, end, is_babble)`.
fn match_segment(
    span: (usize, usize),
    segments: &[(usize, usize, bool)],
) -> Option<(usize, usize)> {
    let len = span.1.saturating_sub(span.0).max(1);
    let mut best: Option<(usize, usize)> = None;
    for (i, (s, e, babble)) in segments.iter().enumerate() {
        if *babble {
            continue;
        }
        let o = overlap_ms(span, (*s, *e));
        if o > 0 && best.is_none_or(|(_, bo)| o > bo) {
            best = Some((i, o));
        }
    }
    best.filter(|(_, o)| o * 2 >= len)
}

/// How late (positive) or early the VAD utterance started relative to the segment, in ms.
fn onset_delay_ms(utt_start_ms: usize, seg_start_ms: usize) -> i64 {
    utt_start_ms as i64 - seg_start_ms as i64
}

/// Onset delay of a turn: the earliest VAD utterance overlapping the turn's span (when the VAD
/// splits a turn, the first piece is what noticed it). `None` when no utterance overlaps.
fn turn_onset_ms(seg_start: usize, seg_end: usize, utts: &[(usize, usize)]) -> Option<i64> {
    utts.iter()
        .filter(|u| overlap_ms(**u, (seg_start, seg_end)) > 0)
        .map(|u| u.0)
        .min()
        .map(|start| onset_delay_ms(start, seg_start))
}

fn median_i64(values: &[i64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let mid = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[mid] as f64
    } else {
        (v[mid - 1] + v[mid]) as f64 / 2.0
    })
}

/// Mono 16 kHz f32 samples of 48 kHz stereo PCM, converted the way the product feeds STT.
fn mono_16k_f32(pcm_48k_stereo: &[u8]) -> Vec<f32> {
    node_webrtc_rust_speech::pcm::stereo_48k_to_mono_16k(pcm_48k_stereo)
        .iter()
        .map(|s| f32::from(*s) / 32_768.0)
        .collect()
}

/// Every preprocessor of the bench, in table order.
const ALL_PRE: [&str; 8] = [
    "raw",
    "rnnoise",
    "gtcrn",
    "dpdfnet_baseline",
    "dpdfnet2",
    "dpdfnet4",
    "dpdfnet8",
    "dpdfnet2_48khz_hr",
];

fn make_preprocessor(name: &str, models: &Path) -> Result<Preprocessor, String> {
    match name {
        "raw" => Ok(Preprocessor::Raw),
        "rnnoise" => Ok(Preprocessor::rnnoise()),
        "gtcrn" => Preprocessor::sherpa(SherpaKind::Gtcrn, &models.join("gtcrn_simple.onnx")),
        "dpdfnet_baseline" | "dpdfnet2" | "dpdfnet4" | "dpdfnet8" | "dpdfnet2_48khz_hr" => {
            Preprocessor::sherpa(SherpaKind::Dpdfnet, &models.join(format!("{name}.onnx")))
        }
        other => Err(format!("unknown preprocessor {other}")),
    }
}

struct Verdict {
    seg: Option<usize>,
    overlap: usize,
    transcript: String,
    caller: bool,
    score: f32,
    span: (usize, usize),
    merged: Vec<usize>,
}

fn words_of(text: &str) -> Vec<String> {
    normalize_for_compare(text)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// A VAD utterance that overlaps a segment.
struct Noticed {
    id: usize,
    start_ms: usize,
    end_ms: usize,
}

/// One line of the per-turn record: a manifest segment (enrollment, main, bystander) or a VAD
/// utterance that matches no segment.
struct Row {
    scene: String,
    level_db: i32,
    vad: String,
    seg_id: Option<usize>,
    start_ms: usize,
    end_ms: usize,
    role: String,
    voice: String,
    sid: Option<i32>,
    kind: String,
    text: String,
    expected_speaker: String,
    expected_transcript: Option<String>,
    utts: Vec<Noticed>,
    transcript: Option<String>,
    score: Option<f32>,
    /// `caller`, `other`, `not noticed` or `enrollment`.
    verdict: String,
    merged: Vec<usize>,
    attribution_ok: Option<bool>,
    wer: Option<f64>,
    word_similarity: Option<f64>,
    oracle_ok: Option<bool>,
    /// Preprocessor of the run (`part2` for the Part 2 path, which has none) and AGC.
    pre: String,
    agc: bool,
}

/// A preprocessor (and optional AGC) in front of the VAD / STT split (Part 3).
struct Enhance<'a> {
    pre_name: &'a str,
    agc: bool,
    models: &'a Path,
    /// Precomputed preprocessor: scene audio comes from `<dir>/scenes/<id>.wav` (already
    /// processed, 48 kHz stereo) and the in-process preprocessor is `raw`.
    scene_dir: Option<&'a Path>,
}

struct P2<'a> {
    enhance: Option<&'a Enhance<'a>>,
    emb: &'a Embedder,
    check_s: f32,
    threshold: f32,
    recognizer: &'a sherpa_onnx::OnlineRecognizer,
    vad_name: &'a str,
    vad_config: &'a VadConfig,
    leak_vocab: &'a HashSet<String>,
}

/// 48 kHz stereo scene audio: the fixture's, or the precomputed preprocessor's when set.
fn scene_audio_path(fixtures: &Path, precomputed: Option<&Path>, id: &str) -> PathBuf {
    precomputed
        .unwrap_or(fixtures)
        .join("scenes")
        .join(format!("{id}.wav"))
}

fn part2_scene(fx: &mut Fixtures, scene: usize, p: &P2) -> Result<(LevelStats, Vec<Row>), String> {
    let id = fx.scenes[scene].id.clone();
    let level_db = fx.scenes[scene].level_db;
    let raw_pcm = read_wav_stereo_48k_pcm(&scene_audio_path(
        &fx.dir,
        p.enhance.and_then(|e| e.scene_dir),
        &id,
    ))?;
    // Part 2: the scene as generated (16 kHz wav for STT and embedding). Part 3: the product
    // order, preprocessor and AGC on the 48 kHz stereo frames first, then VAD on that PCM and
    // STT / embedding on its mono 16 kHz conversion; the enrollment is cut from the same signal.
    let (pcm, audio, underruns, pre_name, agc_on) = match p.enhance {
        None => (raw_pcm, fx.audio16(scene)?.clone(), 0, "part2", false),
        Some(e) => {
            let mut pre = if e.scene_dir.is_some() {
                Preprocessor::Raw
            } else {
                make_preprocessor(e.pre_name, e.models)?
            };
            let mut agc = e.agc.then(Agc::new);
            let processed = process_pcm(&raw_pcm, &mut pre, agc.as_mut());
            let audio = mono_16k_f32(&processed);
            (processed, audio, pre.underruns(), e.pre_name, e.agc)
        }
    };
    let enroll = enrollment_clip(&enrollment_ranges(fx, scene)?, &audio);
    let (reference, _) = p
        .emb
        .embed(&enroll)
        .ok_or("enrollment too short to embed")?;
    let utterances = run_vad(&pcm, p.vad_config)?;
    let pre_roll = pre_roll_ms(p.vad_config) as usize;

    let segs: Vec<Seg> = fx.scenes[scene].segments.clone();
    let seg_info: Vec<(usize, usize, bool)> = segs
        .iter()
        .map(|s| (s.start_ms, s.end_ms, s.role == "babble"))
        .collect();
    let mut stats = LevelStats {
        utterances: utterances.len(),
        underruns,
        ..Default::default()
    };
    let mut verdicts: Vec<Verdict> = Vec::new();
    for utt in &utterances {
        // STT input and embedding clip both start where the product's STT feed starts: the
        // SpeechStart frame minus the pre-roll.
        let from_ms = utt.speech_start_ms.saturating_sub(pre_roll);
        let window_from = from_ms * 16;
        let window_to = (utt.window_end_ms * 16).min(audio.len());
        let mut window = audio[window_from.min(window_to)..window_to].to_vec();
        window.extend(std::iter::repeat_n(0.0f32, TAIL_SILENCE_MS * 16));
        let transcript = recognize(p.recognizer, &window);

        let (from, to) = part2_clip_range(from_ms, utt.window_end_ms, p.check_s);
        let from = from.min(audio.len());
        let clip = &audio[from..to.min(audio.len()).max(from)];
        let (caller, score) = match p.emb.embed(clip) {
            Some((e, _)) => {
                let score = cosine(&reference, &e);
                (score > p.threshold, score)
            }
            None => (false, f32::NAN),
        };

        let span = (utt.start_ms, utt.end_active_ms);
        let matched = match_segment(span, &seg_info);
        if matched.is_none() {
            stats.unmatched += 1;
            if caller {
                stats.unmatched_caller += 1;
            } else {
                stats.unmatched_other += 1;
            }
        }
        let merged = merged_segments(span, &seg_info);
        if !merged.is_empty() {
            stats.merged += 1;
        }
        verdicts.push(Verdict {
            seg: matched.map(|m| m.0),
            overlap: matched.map_or(0, |m| m.1),
            transcript,
            caller,
            score,
            span,
            merged,
        });
    }

    let mut rows: Vec<Row> = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        if seg.role == "babble" {
            continue;
        }
        let mine: Vec<&Verdict> = verdicts.iter().filter(|v| v.seg == Some(i)).collect();
        // Utterances are in time order; the attribution of a turn is its best-overlapping one.
        let best = mine.iter().max_by_key(|v| v.overlap).copied();
        let transcript = (!mine.is_empty()).then(|| {
            mine.iter()
                .map(|v| v.transcript.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        });
        let mut merged: Vec<usize> = Vec::new();
        let mut utts = Vec::new();
        for (u, v) in verdicts.iter().enumerate() {
            if overlap_ms(v.span, (seg.start_ms, seg.end_ms)) > 0 {
                utts.push(Noticed {
                    id: u,
                    start_ms: v.span.0,
                    end_ms: v.span.1,
                });
                merged.extend(v.merged.iter().copied());
            }
        }
        merged.sort_unstable();
        merged.dedup();
        let verdict = if seg.enroll {
            "enrollment"
        } else {
            match best {
                Some(v) if v.caller => "caller",
                Some(_) => "other",
                None => "not noticed",
            }
        };
        let mut row = Row {
            scene: id.clone(),
            level_db,
            vad: p.vad_name.to_string(),
            seg_id: Some(i),
            start_ms: seg.start_ms,
            end_ms: seg.end_ms,
            role: if seg.enroll {
                "main (enrollment)".to_string()
            } else {
                seg.role.clone()
            },
            voice: seg.voice.clone(),
            sid: Some(seg.sid),
            kind: seg.kind.clone(),
            text: seg.text.clone(),
            expected_speaker: seg.expected_speaker.clone(),
            expected_transcript: seg.expected_transcript.clone(),
            utts,
            transcript: transcript.clone(),
            score: best.map(|v| v.score),
            verdict: verdict.to_string(),
            merged,
            attribution_ok: (!seg.enroll).then(|| verdict == seg.expected_speaker),
            wer: None,
            word_similarity: None,
            oracle_ok: None,
            pre: pre_name.to_string(),
            agc: agc_on,
        };
        let long = seg.kind == "long";
        if seg.role == "main" {
            let heard = transcript.clone().unwrap_or_default();
            let sim = word_similarity(&seg.text, &heard);
            let (edits, n) = word_edits(&seg.text, &heard);
            if long {
                row.wer = Some(if n == 0 { 0.0 } else { edits as f64 / n as f64 });
                row.word_similarity = Some(sim);
            } else {
                row.oracle_ok = Some(sim >= ORACLE_MIN);
            }
            if !seg.enroll {
                stats.main_turns += 1;
                if long {
                    stats.l_turns += 1;
                    stats.l_edits += edits;
                    stats.l_ref_words += n;
                    stats.l_sim_sum += sim;
                } else {
                    stats.q_turns += 1;
                    if sim >= ORACLE_MIN {
                        stats.q_recognized += 1;
                    }
                }
                match best {
                    None => stats.main_missed += 1,
                    Some(b) => {
                        let spans: Vec<(usize, usize)> = verdicts.iter().map(|v| v.span).collect();
                        if let Some(d) = turn_onset_ms(seg.start_ms, seg.end_ms, &spans) {
                            stats.onset_delays.push(d);
                        }
                        if b.caller {
                            stats.main_caller += 1;
                            if long {
                                stats.l_caller += 1;
                            } else {
                                stats.q_caller += 1;
                            }
                        }
                        let words = words_of(&heard);
                        stats.main_words += words.len();
                        stats.leak_words +=
                            words.iter().filter(|w| p.leak_vocab.contains(*w)).count();
                    }
                }
            }
        } else {
            stats.byst_turns += 1;
            match best {
                None => stats.byst_missed += 1,
                Some(v) if v.caller => stats.byst_caller += 1,
                Some(_) => stats.byst_other += 1,
            }
        }
        rows.push(row);
    }
    // VAD utterances that matched no segment: babble (or a split turn) that the VAD kept.
    for (u, v) in verdicts.iter().enumerate() {
        if v.seg.is_some() {
            continue;
        }
        let verdict = if v.caller { "caller" } else { "other" };
        rows.push(Row {
            scene: id.clone(),
            level_db,
            vad: p.vad_name.to_string(),
            seg_id: None,
            start_ms: v.span.0,
            end_ms: v.span.1,
            role: "none/babble".to_string(),
            voice: "-".to_string(),
            sid: None,
            kind: "-".to_string(),
            text: String::new(),
            expected_speaker: "other".to_string(),
            expected_transcript: None,
            utts: vec![Noticed {
                id: u,
                start_ms: v.span.0,
                end_ms: v.span.1,
            }],
            transcript: Some(v.transcript.clone()),
            score: Some(v.score),
            verdict: verdict.to_string(),
            merged: v.merged.clone(),
            attribution_ok: Some(verdict == "other"),
            wer: None,
            word_similarity: None,
            oracle_ok: None,
            pre: pre_name.to_string(),
            agc: agc_on,
        });
    }
    rows.sort_by_key(|r| (r.start_ms, r.end_ms));
    Ok((stats, rows))
}

fn add_stats(into: &mut LevelStats, other: &LevelStats) {
    into.main_turns += other.main_turns;
    into.main_missed += other.main_missed;
    into.main_caller += other.main_caller;
    into.q_turns += other.q_turns;
    into.q_recognized += other.q_recognized;
    into.q_caller += other.q_caller;
    into.l_turns += other.l_turns;
    into.l_caller += other.l_caller;
    into.l_edits += other.l_edits;
    into.l_ref_words += other.l_ref_words;
    into.l_sim_sum += other.l_sim_sum;
    into.byst_turns += other.byst_turns;
    into.byst_missed += other.byst_missed;
    into.byst_other += other.byst_other;
    into.byst_caller += other.byst_caller;
    into.leak_words += other.leak_words;
    into.main_words += other.main_words;
    into.unmatched += other.unmatched;
    into.unmatched_caller += other.unmatched_caller;
    into.unmatched_other += other.unmatched_other;
    into.utterances += other.utterances;
    into.merged += other.merged;
    into.onset_delays.extend_from_slice(&other.onset_delays);
    into.underruns += other.underruns;
}

// ---------------------------------------------------------------------------------------------
// Per-turn record (turns.jsonl, conversations.md)
// ---------------------------------------------------------------------------------------------

fn jstr(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

fn jopt_str(s: Option<&str>) -> String {
    s.map_or("null".to_string(), jstr)
}

fn jopt_bool(b: Option<bool>) -> String {
    b.map_or("null".to_string(), |b| b.to_string())
}

fn jopt_f(v: Option<f64>, digits: usize) -> String {
    v.filter(|x| x.is_finite())
        .map_or("null".to_string(), |x| format!("{x:.digits$}"))
}

fn row_json(r: &Row) -> String {
    let utts: Vec<String> = r
        .utts
        .iter()
        .map(|u| {
            format!(
                "{{\"id\": {}, \"start_ms\": {}, \"end_ms\": {}}}",
                u.id, u.start_ms, u.end_ms
            )
        })
        .collect();
    let merged: Vec<String> = r.merged.iter().map(usize::to_string).collect();
    format!(
        "{{\"scene\": {}, \"level_db\": {}, \"vad\": {}, \"pre\": {}, \"agc\": {}, \"segment_id\": {}, \"start_ms\": {}, \"end_ms\": {}, \
         \"who\": {{\"role\": {}, \"voice\": {}, \"sid\": {}}}, \"said\": {{\"kind\": {}, \"text\": {}}}, \
         \"expected\": {{\"speaker\": {}, \"transcript\": {}}}, \
         \"noticed\": {{\"utterances\": [{}], \"transcript\": {}, \"score\": {}, \"verdict\": {}}}, \
         \"merged\": [{}], \
         \"result\": {{\"attribution_ok\": {}, \"wer\": {}, \"word_similarity\": {}, \"oracle_ok\": {}}}}}",
        jstr(&r.scene),
        r.level_db,
        jstr(&r.vad),
        jstr(&r.pre),
        r.agc,
        r.seg_id.map_or("null".to_string(), |i| i.to_string()),
        r.start_ms,
        r.end_ms,
        jstr(&r.role),
        jstr(&r.voice),
        r.sid.map_or("null".to_string(), |i| i.to_string()),
        jstr(&r.kind),
        jstr(&r.text),
        jstr(&r.expected_speaker),
        jopt_str(r.expected_transcript.as_deref()),
        utts.join(", "),
        jopt_str(r.transcript.as_deref()),
        jopt_f(r.score.map(f64::from), 3),
        jstr(&r.verdict),
        merged.join(", "),
        jopt_bool(r.attribution_ok),
        jopt_f(r.wer, 4),
        jopt_f(r.word_similarity, 4),
        jopt_bool(r.oracle_ok),
    )
}

fn md_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn mark(ok: Option<bool>) -> &'static str {
    match ok {
        Some(true) => "✓",
        Some(false) => "✗",
        None => "-",
    }
}

fn secs(ms: usize) -> String {
    format!("{:.1}", ms as f64 / 1000.0)
}

/// One timeline table per (VAD variant, scene). Scores and transcripts are bench data from
/// synthetic voices, not customer content.
fn conversations_md(rows: &[Row]) -> String {
    let mut md = String::from(
        "# Speaker bench: conversations\n\nOne table per scene and VAD variant. `noticed` is what the \
         VAD heard and what the recognizer transcribed for it; `score` is the speaker score against \
         the enrollment (higher = more like the caller). The last column is attribution (verdict equals \
         expected speaker); a qualified phrase adds the oracle result, a long sentence its WER.\n\n",
    );
    // Part 3 rows are many; only raw/off, gtcrn/on and dpdfnet2/on get a timeline (the rest are
    // in turns.jsonl).
    let shown = |r: &Row| {
        r.pre == "part2"
            || (r.pre == "raw" && !r.agc)
            || (r.pre == "gtcrn" && r.agc)
            || (r.pre == "dpdfnet2" && r.agc)
            || (r.pre == "voxlock-live" && r.agc)
    };
    let mut keys: Vec<(String, bool, String, String)> = Vec::new();
    for r in rows.iter().filter(|r| shown(r)) {
        let key = (r.pre.clone(), r.agc, r.vad.clone(), r.scene.clone());
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for (pre, agc, vad, scene) in keys {
        let scene_rows: Vec<&Row> = rows
            .iter()
            .filter(|r| r.pre == pre && r.agc == agc && r.vad == vad && r.scene == scene)
            .collect();
        let pipeline = if pre == "part2" {
            String::new()
        } else {
            format!(", pre {pre}, AGC {}", if agc { "on" } else { "off" })
        };
        md.push_str(&format!(
            "## {scene} ({} dB), VAD {vad}{pipeline}\n\n| time | who spoke | said | expected | noticed (transcript) | verdict | ✓/✗ |\n|---|---|---|---|---|---|---|\n",
            scene_rows[0].level_db
        ));
        for r in scene_rows {
            let who = match r.sid {
                Some(sid) => format!("{} {} (sid {sid})", r.role, r.voice),
                None => r.role.clone(),
            };
            let said = if r.text.is_empty() {
                "-".to_string()
            } else {
                format!("[{}] {}", r.kind, r.text)
            };
            let mut noticed = if r.utts.is_empty() {
                "no VAD utterance".to_string()
            } else {
                let ids: Vec<String> = r
                    .utts
                    .iter()
                    .map(|u| format!("utt#{} {}-{} s", u.id, secs(u.start_ms), secs(u.end_ms)))
                    .collect();
                format!(
                    "{}: \"{}\"",
                    ids.join(", "),
                    r.transcript.as_deref().unwrap_or("")
                )
            };
            if let Some(score) = r.score.filter(|s| s.is_finite()) {
                noticed.push_str(&format!(" (score {score:.3})"));
            }
            if !r.merged.is_empty() {
                let ids: Vec<String> = r.merged.iter().map(usize::to_string).collect();
                noticed.push_str(&format!(" MERGED with segments [{}]", ids.join(", ")));
            }
            let mut result = mark(r.attribution_ok).to_string();
            if let Some(w) = r.wer {
                result.push_str(&format!(", WER {:.0}%", w * 100.0));
            }
            if r.oracle_ok.is_some() {
                result.push_str(&format!(", oracle {}", mark(r.oracle_ok)));
            }
            md.push_str(&format!(
                "| {}-{} s | {} | {} | {} | {} | {} | {} |\n",
                secs(r.start_ms),
                secs(r.end_ms),
                md_cell(&who),
                md_cell(&said),
                md_cell(&format!(
                    "{} ({})",
                    r.expected_speaker,
                    r.expected_transcript
                        .as_deref()
                        .map_or("no transcript", |_| "same text")
                )),
                md_cell(&noticed),
                md_cell(&r.verdict),
                md_cell(&result)
            ));
        }
        md.push('\n');
    }
    md
}

// ---------------------------------------------------------------------------------------------
// Environment facts
// ---------------------------------------------------------------------------------------------

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn cpu_model() -> String {
    if let Ok(info) = fs::read_to_string("/proc/cpuinfo") {
        if let Some(line) = info.lines().find(|l| l.starts_with("model name")) {
            return line.split(':').nth(1).unwrap_or("").trim().to_string();
        }
    }
    command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
        .unwrap_or_else(|| "unknown".into())
}

fn git_sha() -> String {
    std::env::var("NWR_BENCH_GIT_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| command_output("git", &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".into())
}

// ---------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Part 3: denoisers and caller AGC
// ---------------------------------------------------------------------------------------------

/// Preprocessors of Grid B (caller level).
const GRID_B_PRE: [&str; 4] = ["raw", "rnnoise", "gtcrn", "dpdfnet2"];
/// Streams-per-core steps.
const STREAM_STEPS: [usize; 8] = [1, 2, 4, 8, 12, 16, 24, 32];
/// Frames at the start of a cost run that are not counted (model warm-up).
const COST_WARMUP_FRAMES: usize = 25;
/// `level_db` of the babble-free S1 scenes.
const CLEAN_LEVEL_DB: i32 = -99;

struct Part3Run<'a> {
    models: &'a Path,
    pre_filter: &'a [String],
    /// `(name, dir)` of every `--pre precomputed:<name>=<dir>`.
    precomputed: &'a [(String, PathBuf)],
    emb: &'a Embedder,
    check_s: f32,
    threshold: f32,
    recognizer: &'a sherpa_onnx::OnlineRecognizer,
    vad_name: &'a str,
    vad_config: &'a VadConfig,
    leak_vocab: &'a HashSet<String>,
    /// The cost and S1 WER tables do not depend on the VAD: only the first VAD run computes them.
    with_cost: bool,
    cost_seconds: usize,
    streams_seconds: usize,
}

struct GridRow {
    pre: String,
    agc: bool,
    main_db: i32,
    /// Caller level the row is labelled with: the mean `effective_main_db` of its scenes in 0.5 dB
    /// steps (the nominal `main_db` for manifests without it).
    main_label: f64,
    level_db: i32,
    stats: LevelStats,
}

struct CostRow {
    /// Preprocessor name, or `agc` for the AGC alone.
    name: String,
    frames: usize,
    rtf_p50: f64,
    rtf_p99: f64,
    rtf_mean: f64,
    /// Largest N of [`STREAM_STEPS`] with at most 1 % late frames; `None` when not measured.
    streams: Option<usize>,
    /// The largest step passed, and it was the last step (the real value may be higher).
    streams_capped: bool,
    /// `(N, late share)` of every step that ran.
    stream_steps: Vec<(usize, f64)>,
    delay_ms: Option<f64>,
}

#[derive(Default, Clone)]
struct WerStats {
    q_turns: usize,
    q_ok: usize,
    l_turns: usize,
    l_edits: usize,
    l_words: usize,
}

struct WerRow {
    pre: String,
    level_db: i32,
    stats: WerStats,
}

struct Part3 {
    vad: String,
    model: String,
    check_s: f32,
    threshold: f32,
    set: &'static str,
    pre_names: Vec<String>,
    /// Names of the precomputed preprocessors (a subset of `pre_names`).
    precomputed: Vec<String>,
    grid: Vec<GridRow>,
    cost: Vec<CostRow>,
    wer: Vec<WerRow>,
}

fn percentile_f(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

/// 20 ms frames of the S3 audio (48 kHz stereo), at least `seconds` worth (cycled if shorter).
fn cost_frames(fx: &Fixtures, scenes: &[usize], seconds: usize) -> Result<Vec<Vec<u8>>, String> {
    let wanted = seconds * 50;
    let mut frames: Vec<Vec<u8>> = Vec::new();
    for &scene in scenes {
        let pcm = read_wav_stereo_48k_pcm(
            &fx.dir
                .join("scenes")
                .join(format!("{}.wav", fx.scenes[scene].id)),
        )?;
        frames.extend(
            pcm.as_chunks::<{ enhance::FRAME_BYTES }>()
                .0
                .iter()
                .map(|frame| frame.to_vec()),
        );
        if frames.len() >= wanted + COST_WARMUP_FRAMES {
            break;
        }
    }
    if frames.is_empty() {
        return Err("part 3 cost: no audio".into());
    }
    let mut out = Vec::new();
    while out.len() < wanted + COST_WARMUP_FRAMES {
        let i = out.len() % frames.len();
        out.push(frames[i].clone());
    }
    Ok(out)
}

/// Per-frame RTF of one stream, with `agc_alone` timing only the AGC.
fn single_stream_rtf(
    name: &str,
    models: &Path,
    agc_alone: bool,
    frames: &[Vec<u8>],
) -> Result<Vec<f64>, String> {
    let mut pre = make_preprocessor(if agc_alone { "raw" } else { name }, models)?;
    let mut agc = Agc::new();
    let mut rtf = Vec::new();
    for (i, frame) in frames.iter().enumerate() {
        let start = Instant::now();
        let out = if agc_alone {
            agc.process(frame)
        } else {
            pre.process(frame)
        };
        let elapsed = start.elapsed().as_secs_f64();
        std::hint::black_box(&out);
        if i >= COST_WARMUP_FRAMES {
            rtf.push(elapsed / 0.020);
        }
    }
    Ok(rtf)
}

/// Share of frames that finished more than 20 ms after they were due, with `n` real-time paced
/// streams on this thread (one frame per stream every 20 ms).
fn late_share(
    name: &str,
    models: &Path,
    agc_alone: bool,
    n: usize,
    frames: &[Vec<u8>],
    seconds: usize,
) -> Result<f64, String> {
    let mut pres = Vec::new();
    let mut agcs = Vec::new();
    for _ in 0..n {
        pres.push(make_preprocessor(
            if agc_alone { "raw" } else { name },
            models,
        )?);
        agcs.push(Agc::new());
    }
    let ticks = seconds * 50 + COST_WARMUP_FRAMES;
    let period = std::time::Duration::from_millis(20);
    let start = Instant::now();
    let (mut late, mut total) = (0usize, 0usize);
    for k in 0..ticks {
        let due = start + period * k as u32;
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        }
        for i in 0..n {
            let frame = &frames[(k + i * 37) % frames.len()];
            let out = if agc_alone {
                agcs[i].process(frame)
            } else {
                pres[i].process(frame)
            };
            std::hint::black_box(&out);
            if k >= COST_WARMUP_FRAMES {
                total += 1;
                if Instant::now() > due + period {
                    late += 1;
                }
            }
        }
    }
    Ok(late as f64 / total.max(1) as f64)
}

/// Added delay of a preprocessor in ms: the lag (0 to 800 ms, at 8 kHz) that maximizes the
/// cross-correlation of the input and the processed 16 kHz mono signal.
fn delay_ms(pre_name: &str, models: &Path, pcm: &[u8]) -> Result<f64, String> {
    let mut pre = make_preprocessor(pre_name, models)?;
    let processed = process_pcm(pcm, &mut pre, None);
    let decimate = |x: Vec<f32>| -> Vec<f32> {
        x.as_chunks::<2>()
            .0
            .iter()
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect()
    };
    let x = decimate(mono_16k_f32(pcm));
    let y = decimate(mono_16k_f32(&processed));
    let n = x.len().min(y.len()).min(48_000);
    let mut best = (0usize, f64::NEG_INFINITY);
    for lag in 0..=6_400usize.min(n.saturating_sub(1)) {
        let m = n - lag;
        let dot: f64 = (0..m)
            .map(|i| f64::from(x[i]) * f64::from(y[i + lag]))
            .sum();
        if dot > best.1 {
            best = (lag, dot);
        }
    }
    Ok(best.0 as f64 / 8.0)
}

/// Scene for the added-delay measurement: a clean S1 clip, else an S1 babble clip.
fn delay_scene(kinds: &[&str]) -> Option<usize> {
    kinds
        .iter()
        .position(|k| *k == "main_clean")
        .or_else(|| kinds.iter().position(|k| *k == "main_over_babble"))
}

fn cost_rows(
    fx: &Fixtures,
    r: &Part3Run,
    pre_names: &[&str],
    s3: &[usize],
) -> Result<Vec<CostRow>, String> {
    let frames = cost_frames(fx, s3, r.cost_seconds)?;
    // Input for the delay measurement: a clean S1 clip (babble clip if none), at most 6 s.
    let kinds: Vec<&str> = fx.scenes.iter().map(|s| s.kind.as_str()).collect();
    let delay_scene = delay_scene(&kinds)
        .or_else(|| s3.first().copied())
        .ok_or("part 3: no scene for the delay measurement")?;
    let delay_pcm: Vec<u8> = read_wav_stereo_48k_pcm(
        &fx.dir
            .join("scenes")
            .join(format!("{}.wav", fx.scenes[delay_scene].id)),
    )?
    .into_iter()
    .take(6 * 48_000 * 4)
    .collect();

    let mut names: Vec<(String, bool)> = pre_names
        .iter()
        .map(|p| ((*p).to_string(), false))
        .collect();
    names.push(("agc".to_string(), true));
    let mut out = Vec::new();
    for (name, agc_alone) in names {
        eprintln!("part 3 cost: {name}");
        let mut rtf = single_stream_rtf(&name, r.models, agc_alone, &frames)?;
        let mean = rtf.iter().sum::<f64>() / rtf.len().max(1) as f64;
        rtf.sort_by(|a, b| a.total_cmp(b));
        let mut stream_steps = Vec::new();
        let mut streams = None;
        let mut capped = false;
        if name != "raw" {
            let mut best = 0usize;
            for (i, &n) in STREAM_STEPS.iter().enumerate() {
                let late = late_share(&name, r.models, agc_alone, n, &frames, r.streams_seconds)?;
                eprintln!("part 3 cost: {name} x{n}: {:.2} % late", late * 100.0);
                stream_steps.push((n, late));
                if late <= 0.01 {
                    best = n;
                    capped = i + 1 == STREAM_STEPS.len();
                } else {
                    break;
                }
            }
            streams = Some(best);
        }
        let delay = if agc_alone || name == "raw" {
            Some(0.0)
        } else {
            Some(delay_ms(&name, r.models, &delay_pcm)?)
        };
        out.push(CostRow {
            name,
            frames: rtf.len(),
            rtf_p50: percentile_f(&rtf, 0.5),
            rtf_p99: percentile_f(&rtf, 0.99),
            rtf_mean: mean,
            streams,
            streams_capped: capped,
            stream_steps,
            delay_ms: delay,
        });
    }
    Ok(out)
}

/// S1 whole-clip WER: the main clip (no VAD) through the preprocessor and the online recognizer.
fn s1_wer(fx: &Fixtures, r: &Part3Run, pre_names: &[&str]) -> Result<Vec<WerRow>, String> {
    let s1 = |i: &usize| {
        matches!(
            fx.scenes[*i].kind.as_str(),
            "main_over_babble" | "main_clean"
        )
    };
    let all: Vec<usize> = (0..fx.scenes.len()).filter(s1).collect();
    let set = if all.iter().any(|i| fx.scenes[*i].set == Set::Test) {
        Set::Test
    } else {
        Set::Tune
    };
    let scenes: Vec<usize> = all
        .into_iter()
        .filter(|i| fx.scenes[*i].set == set)
        .collect();
    let mut rows = Vec::new();
    for name in pre_names {
        eprintln!("part 3 S1 WER: {name} ({} scenes)", scenes.len());
        let mut by_level: BTreeMap<i32, WerStats> = BTreeMap::new();
        for &scene in &scenes {
            let info = &fx.scenes[scene];
            let Some(seg) = info.segments.iter().find(|s| s.role == "main") else {
                continue;
            };
            let pcm =
                read_wav_stereo_48k_pcm(&fx.dir.join("scenes").join(format!("{}.wav", info.id)))?;
            let mut pre = make_preprocessor(name, r.models)?;
            let processed = process_pcm(&pcm, &mut pre, None);
            let mut window = mono_16k_f32(&processed);
            window.extend(std::iter::repeat_n(0.0f32, TAIL_SILENCE_MS * 16));
            let heard = recognize(r.recognizer, &window);
            let stats = by_level.entry(info.level_db).or_default();
            if seg.kind == "long" {
                let (edits, n) = word_edits(&seg.text, &heard);
                stats.l_turns += 1;
                stats.l_edits += edits;
                stats.l_words += n;
            } else {
                stats.q_turns += 1;
                if word_similarity(&seg.text, &heard) >= ORACLE_MIN {
                    stats.q_ok += 1;
                }
            }
        }
        for (level_db, stats) in by_level {
            rows.push(WerRow {
                pre: (*name).to_string(),
                level_db,
                stats,
            });
        }
    }
    Ok(rows)
}

/// Cost row of a precomputed preprocessor from `<dir>/voxlock_cost.json`
/// (`{"scenes":[{"id":..,"audio_s":..,"wall_s":..,"cpu_s":..}]}`, written under `taskset -c 2`).
/// The RTF columns hold process CPU seconds per audio second over the scenes.
fn precomputed_cost_row(name: &str, dir: &Path) -> Result<CostRow, String> {
    let path = dir.join("voxlock_cost.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    cost_row_from_json(name, &text).map_err(|e| format!("{}: {e}", path.display()))
}

fn cost_row_from_json(name: &str, text: &str) -> Result<CostRow, String> {
    let json = parse_json(text)?;
    let Some(Json::Arr(scenes)) = json.get("scenes") else {
        return Err("no scenes array".into());
    };
    let mut ratios = Vec::new();
    let mut audio_total = 0.0;
    for s in scenes {
        let num = |key: &str| match s.get(key) {
            Some(Json::Num(n)) => *n,
            _ => 0.0,
        };
        let (audio, cpu) = (num("audio_s"), num("cpu_s"));
        if audio > 0.0 {
            ratios.push(cpu / audio);
            audio_total += audio;
        }
    }
    if ratios.is_empty() {
        return Err("no scene with audio".into());
    }
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    ratios.sort_by(|a, b| a.total_cmp(b));
    Ok(CostRow {
        name: name.to_string(),
        frames: (audio_total * 50.0).round() as usize,
        rtf_p50: percentile_f(&ratios, 0.5),
        rtf_p99: percentile_f(&ratios, 0.99),
        rtf_mean: mean,
        streams: None,
        streams_capped: false,
        stream_steps: Vec::new(),
        delay_ms: None,
    })
}

/// The precomputed preprocessors whose `scenes/` dir and `voxlock_cost.json` exist. A missing one
/// is warned about and dropped, so a failed VoxLock step does not cost the other results.
fn usable_precomputed(all: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    all.iter()
        .filter(|(name, dir)| {
            let ok = dir.join("scenes").is_dir() && dir.join("voxlock_cost.json").is_file();
            if !ok {
                eprintln!(
                    "warning: precomputed {name}: {} has no scenes/ or voxlock_cost.json; skipped",
                    dir.display()
                );
            }
            ok
        })
        .cloned()
        .collect()
}

fn part3_run(fx: &mut Fixtures, r: &Part3Run, rows: &mut Vec<Row>) -> Result<Part3, String> {
    let pre_names: Vec<&str> = ALL_PRE
        .iter()
        .copied()
        .filter(|p| r.pre_filter.iter().any(|f| f == p))
        .collect();
    let precomputed = usable_precomputed(r.precomputed);
    if pre_names.is_empty() && precomputed.is_empty() {
        return Err("part 3: --pre selects no preprocessor".into());
    }
    // Fail early on a missing model.
    for name in &pre_names {
        eprintln!(
            "part 3: {name} -> {}",
            make_preprocessor(name, r.models)?.describe()
        );
    }
    let s3: Vec<usize> = (0..fx.scenes.len())
        .filter(|i| fx.scenes[*i].kind == "conversation")
        .collect();
    let set = if s3.iter().any(|i| fx.scenes[*i].set == Set::Test) {
        Set::Test
    } else {
        Set::Tune
    };
    let set_label = if set == Set::Test {
        "test"
    } else {
        "tune (no test-set conversation scene in the fixtures: smoke only)"
    };
    let scenes: Vec<usize> = s3
        .iter()
        .copied()
        .filter(|i| fx.scenes[*i].set == set)
        .collect();

    // Every combo: (name, precomputed scene dir). Precomputed combos run Grid A and B scenes.
    let mut combos: Vec<(&str, Option<&Path>)> = pre_names.iter().map(|n| (*n, None)).collect();
    combos.extend(
        precomputed
            .iter()
            .map(|(n, d)| (n.as_str(), Some(d.as_path()))),
    );
    let mut grid = Vec::new();
    for (name, scene_dir) in &combos {
        for agc in [false, true] {
            let in_b = scene_dir.is_some() || GRID_B_PRE.contains(name);
            let mut by: BTreeMap<(i32, i32), (LevelStats, f64, usize)> = BTreeMap::new();
            let combo_started = std::time::Instant::now();
            for &scene in &scenes {
                let main_db = fx.scenes[scene].main_db;
                let level = fx.scenes[scene].level_db;
                let in_a = main_db == -20;
                let in_b_scene = in_b && (level == -15 || level == -10);
                if !(in_a || in_b_scene) {
                    continue;
                }
                let enhance = Enhance {
                    pre_name: name,
                    agc,
                    models: r.models,
                    scene_dir: *scene_dir,
                };
                let p2 = P2 {
                    enhance: Some(&enhance),
                    emb: r.emb,
                    check_s: r.check_s,
                    threshold: r.threshold,
                    recognizer: r.recognizer,
                    vad_name: r.vad_name,
                    vad_config: r.vad_config,
                    leak_vocab: r.leak_vocab,
                };
                eprintln!(
                    "part 3 pre {name} agc {} scene {}",
                    if agc { "on" } else { "off" },
                    fx.scenes[scene].id
                );
                let (stats, scene_rows) = part2_scene(fx, scene, &p2)?;
                let entry = by.entry((main_db, level)).or_default();
                add_stats(&mut entry.0, &stats);
                if let Some(e) = fx.scenes[scene].effective_main_db {
                    entry.1 += e;
                    entry.2 += 1;
                }
                rows.extend(scene_rows);
            }
            eprintln!(
                "part 3 pre {name} agc {} done in {:.0} s",
                on_off(agc),
                combo_started.elapsed().as_secs_f64()
            );
            for ((main_db, level_db), (stats, eff_sum, eff_n)) in by {
                grid.push(GridRow {
                    pre: (*name).to_string(),
                    agc,
                    main_db,
                    main_label: effective_label(eff_sum, eff_n, main_db),
                    level_db,
                    stats,
                });
            }
        }
    }
    let wer = if r.with_cost {
        s1_wer(fx, r, &pre_names)?
    } else {
        Vec::new()
    };
    let mut cost = if r.with_cost {
        cost_rows(fx, r, &pre_names, &scenes)?
    } else {
        Vec::new()
    };
    if r.with_cost {
        for (name, dir) in &precomputed {
            cost.push(precomputed_cost_row(name, dir)?);
        }
    }
    Ok(Part3 {
        vad: r.vad_name.to_string(),
        model: r.emb.id.clone(),
        check_s: r.check_s,
        threshold: r.threshold,
        set: set_label,
        pre_names: combos.iter().map(|(p, _)| (*p).to_string()).collect(),
        precomputed: precomputed.iter().map(|(n, _)| n.clone()).collect(),
        grid,
        cost,
        wer,
    })
}

fn on_off(agc: bool) -> &'static str {
    if agc {
        "on"
    } else {
        "off"
    }
}

/// The metric cells shared by every Part 3 table row.
fn stats_cells(s: &LevelStats) -> String {
    format!(
        "{} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}",
        s.utterances,
        s.merged,
        s.main_missed,
        s.byst_missed,
        fmt_opt(pct(s.q_recognized, s.q_turns), 1),
        fmt_opt(pct(s.l_edits, s.l_ref_words), 1),
        fmt_opt(pct(s.main_caller, s.main_turns), 1),
        fmt_opt(pct(s.byst_other, s.byst_turns), 1),
        fmt_opt(pct(s.leak_words, s.main_words), 2),
        s.unmatched,
        fmt_opt(median_i64(&s.onset_delays), 0),
        s.underruns,
    )
}

const STATS_HEAD: &str = "VAD utt | merged utt | missed main | missed bystander | (a) oracle % (qualified) | WER % (long) | (b) main to caller % | (c) bystander to other % | (e) leakage % | (f) unmatched utt | median onset ms | underruns";

/// Grid B label: the mean effective caller level of the pooled scenes, in 0.5 dB steps.
fn effective_label(sum: f64, n: usize, nominal: i32) -> f64 {
    if n == 0 {
        return f64::from(nominal);
    }
    (sum / n as f64 * 2.0).round() / 2.0
}

fn pooled(grid: &[GridRow], pre: &str, agc: bool, main_db: i32) -> LevelStats {
    let mut all = LevelStats::default();
    for g in grid
        .iter()
        .filter(|g| g.pre == pre && g.agc == agc && g.main_db == main_db)
    {
        add_stats(&mut all, &g.stats);
    }
    all
}

/// Caption of the cost table; precomputed rows hold a different quantity and say so.
fn cost_caption(precomputed: &[String]) -> String {
    let mut c = String::from("\n### Cost (one core, 20 ms frames)\n\nRTF = processing time / 20 ms per frame. Streams per core = the largest N (steps 1, 2, 4, 8, 12, 16, 24, 32) of real-time paced streams on one core with at most 1 % of frames finishing more than 20 ms after they were due; `+` marks the last step passed. Delay = lag of the cross-correlation of input and output (0 to 800 ms). `agc` is the AGC alone.\n\n");
    if !precomputed.is_empty() {
        c.push_str(&format!(
            " Rows {} are precomputed (VoxLock, `voxlock_preprocess.py` under `taskset -c 2`): the RTF columns hold process CPU seconds per audio second over whole scenes (\u{2248} CPU cores per call), not per 20 ms frame; `frames` is audio seconds \u{d7} 50; streams per core and delay are not measured.",
            precomputed.join(", ")
        ));
    }
    c
}

fn part3_md(p: &Part3) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "\n## Part 3: denoisers and caller AGC, VAD {}\n\nSpeaker model `{}`, check {} s, threshold {:.4} (tune set, FAR 1 %). Scenes: {}. Pipeline per combo: preprocessor, then AGC if on, on the 20 ms 48 kHz stereo frames; the VAD named in the heading on that PCM; STT and embedding on its mono 16 kHz conversion, starting at SpeechStart minus the product pre-roll. `turns.jsonl` has every turn (`pre`, `agc`); `conversations.md` shows raw/off, gtcrn/on, dpdfnet2/on and voxlock-live/on. A precomputed preprocessor (`voxlock-*`) reads scene audio that `benchmarks/speaker/voxlock_preprocess.py` wrote; its cost row comes from `voxlock_cost.json` (process CPU seconds per audio second on one pinned core; RTF columns are that ratio, not per 20 ms frame; streams and delay are not measured).\n",
        p.vad, p.model, p.check_s, p.threshold, p.set
    ));
    md.push_str("\n### Grid A: preprocessor x AGC (S3 scenes at main -20 dBFS)\n\n");
    md.push_str(&format!("| pre | AGC | level | {STATS_HEAD} |\n"));
    md.push_str(&format!("|{}\n", "---|".repeat(15)));
    for name in &p.pre_names {
        for agc in [false, true] {
            for g in p
                .grid
                .iter()
                .filter(|g| g.pre == *name && g.agc == agc && g.main_db == -20)
            {
                md.push_str(&format!(
                    "| {name} | {} | {} dB | {} |\n",
                    on_off(agc),
                    g.level_db,
                    stats_cells(&g.stats)
                ));
            }
            let all = pooled(&p.grid, name, agc, -20);
            md.push_str(&format!(
                "| **{name}** | **{}** | **all** | {} |\n",
                on_off(agc),
                stats_cells(&all)
            ));
        }
    }
    md.push_str("\n### Grid B: caller level (relative levels -15 and -10 dB)\n\n");
    md.push_str(&format!(
        "| pre | AGC | main dBFS | level | {STATS_HEAD} |\n"
    ));
    md.push_str(&format!("|{}\n", "---|".repeat(16)));
    for name in p
        .pre_names
        .iter()
        .filter(|n| GRID_B_PRE.contains(&n.as_str()) || p.precomputed.contains(n))
    {
        for agc in [false, true] {
            for g in p.grid.iter().filter(|g| {
                g.pre == *name && g.agc == agc && (g.level_db == -15 || g.level_db == -10)
            }) {
                md.push_str(&format!(
                    "| {name} | {} | {:.1} | {} dB | {} |\n",
                    on_off(agc),
                    g.main_label,
                    g.level_db,
                    stats_cells(&g.stats)
                ));
            }
        }
    }
    md.push_str("\n### S1 whole-clip WER (main voice over babble, no VAD, AGC off)\n\nQualified phrases: share judged right by the 0.75 oracle; long sentences: WER (measured, not qualified). `clean` = no babble.\n\n");
    md.push_str("| pre | level | qualified clips | oracle % | long clips | WER % (long) |\n|---|---|---|---|---|---|\n");
    for w in &p.wer {
        let level = if w.level_db == CLEAN_LEVEL_DB {
            "clean".to_string()
        } else {
            format!("{} dB", w.level_db)
        };
        md.push_str(&format!(
            "| {} | {level} | {} | {} | {} | {} |\n",
            w.pre,
            w.stats.q_turns,
            fmt_opt(pct(w.stats.q_ok, w.stats.q_turns), 1),
            w.stats.l_turns,
            fmt_opt(pct(w.stats.l_edits, w.stats.l_words), 1),
        ));
    }
    md.push_str(&cost_caption(&p.precomputed));
    md.push_str("| pre | frames | RTF p50 | RTF p99 | RTF mean | streams per core | delay ms |\n|---|---|---|---|---|---|---|\n");
    for c in &p.cost {
        let na = if p.precomputed.contains(&c.name) {
            "n/a"
        } else {
            "-"
        };
        md.push_str(&format!(
            "| {} | {} | {:.4} | {:.4} | {:.4} | {} | {} |\n",
            c.name,
            c.frames,
            c.rtf_p50,
            c.rtf_p99,
            c.rtf_mean,
            c.streams.map_or(na.to_string(), |n| format!(
                "{n}{}",
                if c.streams_capped { "+" } else { "" }
            )),
            c.delay_ms.map_or(na.to_string(), |d| fmt_opt(Some(d), 1)),
        ));
    }
    md
}

fn stats_json(s: &LevelStats) -> String {
    format!(
        "\"vad_utterances\": {}, \"merged_utterances\": {}, \"main_turns\": {}, \"main_missed\": {}, \"qualified_turns\": {}, \"a_qualified_oracle_pct\": {}, \"long_turns\": {}, \"long_wer_pct\": {}, \"b_main_caller_pct\": {}, \"bystander_turns\": {}, \"bystander_missed\": {}, \"c_bystander_other_pct\": {}, \"d_bystander_caller_pct\": {}, \"e_leakage_pct\": {}, \"f_unmatched_utterances\": {}, \"median_onset_delay_ms\": {}, \"underruns\": {}",
        s.utterances,
        s.merged,
        s.main_turns,
        s.main_missed,
        s.q_turns,
        json_num(pct(s.q_recognized, s.q_turns)),
        s.l_turns,
        json_num(pct(s.l_edits, s.l_ref_words)),
        json_num(pct(s.main_caller, s.main_turns)),
        s.byst_turns,
        s.byst_missed,
        json_num(pct(s.byst_other, s.byst_turns)),
        json_num(pct(s.byst_caller, s.byst_turns)),
        json_num(pct(s.leak_words, s.main_words)),
        s.unmatched,
        json_num(median_i64(&s.onset_delays)),
        s.underruns,
    )
}

fn part3_json(p: &Part3, key: &str) -> String {
    let mut j = String::new();
    j.push_str(&format!(
        "  \"{key}\": {{\n    \"model\": \"{}\",\n    \"check_s\": {},\n    \"threshold\": {:.6},\n    \"set\": \"{}\",\n    \"grid\": [\n",
        json_escape(&p.model),
        p.check_s,
        p.threshold,
        json_escape(p.set)
    ));
    let lines: Vec<String> = p
        .grid
        .iter()
        .map(|g| {
            format!(
                "      {{\"pre\": \"{}\", \"agc\": {}, \"main_db\": {}, \"effective_main_db\": {:.1}, \"level_db\": {}, {}}}",
                g.pre,
                g.agc,
                g.main_db,
                g.main_label,
                g.level_db,
                stats_json(&g.stats)
            )
        })
        .collect();
    j.push_str(&lines.join(",\n"));
    j.push_str("\n    ],\n    \"grid_a_pooled\": [\n");
    let mut pooled_lines = Vec::new();
    for name in &p.pre_names {
        for agc in [false, true] {
            pooled_lines.push(format!(
                "      {{\"pre\": \"{name}\", \"agc\": {agc}, \"main_db\": -20, {}}}",
                stats_json(&pooled(&p.grid, name, agc, -20))
            ));
        }
    }
    j.push_str(&pooled_lines.join(",\n"));
    j.push_str("\n    ],\n    \"s1_wer\": [\n");
    let wer_lines: Vec<String> = p
        .wer
        .iter()
        .map(|w| {
            format!(
                "      {{\"pre\": \"{}\", \"level_db\": {}, \"clean\": {}, \"qualified_clips\": {}, \"oracle_pct\": {}, \"long_clips\": {}, \"long_wer_pct\": {}}}",
                w.pre,
                w.level_db,
                w.level_db == CLEAN_LEVEL_DB,
                w.stats.q_turns,
                json_num(pct(w.stats.q_ok, w.stats.q_turns)),
                w.stats.l_turns,
                json_num(pct(w.stats.l_edits, w.stats.l_words)),
            )
        })
        .collect();
    j.push_str(&wer_lines.join(",\n"));
    j.push_str("\n    ],\n    \"cost\": [\n");
    let cost_lines: Vec<String> = p
        .cost
        .iter()
        .map(|c| {
            let steps: Vec<String> = c
                .stream_steps
                .iter()
                .map(|(n, late)| format!("{{\"streams\": {n}, \"late_share\": {late:.5}}}"))
                .collect();
            format!(
                "      {{\"name\": \"{}\", \"frames\": {}, \"rtf_p50\": {:.5}, \"rtf_p99\": {:.5}, \"rtf_mean\": {:.5}, \"streams_per_core\": {}, \"streams_capped\": {}, \"delay_ms\": {}, \"stream_steps\": [{}]}}",
                c.name,
                c.frames,
                c.rtf_p50,
                c.rtf_p99,
                c.rtf_mean,
                c.streams.map_or("null".to_string(), |n| n.to_string()),
                c.streams_capped,
                json_num(c.delay_ms),
                steps.join(", ")
            )
        })
        .collect();
    j.push_str(&cost_lines.join(",\n"));
    j.push_str("\n    ]\n  }");
    j
}

struct VariantOutcome {
    name: String,
    min_silence_ms: u32,
    pre_roll_ms: u32,
    levels: BTreeMap<i32, LevelStats>,
}

/// One adaptive-VAD sweep candidate on the tune scenes.
struct SweepRow {
    margin_db: f32,
    use_speech_level: bool,
    main_turns: usize,
    detected: usize,
    merged: usize,
    unmatched: usize,
}

const SWEEP_MARGINS: [f32; 4] = [4.0, 6.0, 8.0, 10.0];

/// The candidate with the most detected main turns whose `merged + unmatched` does not exceed
/// the energy VAD's `bound`; ties go to the larger margin, then `useSpeechLevel = false`.
/// `None` when no candidate stays within the bound.
fn pick_sweep(rows: &[SweepRow], bound: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.merged + r.unmatched <= bound)
        .max_by(|(_, a), (_, b)| {
            a.detected
                .cmp(&b.detected)
                .then(a.margin_db.total_cmp(&b.margin_db))
                .then(b.use_speech_level.cmp(&a.use_speech_level))
        })
        .map(|(i, _)| i)
}

fn adaptive_vad_config(base: &VadConfig, margin_db: f32, use_speech_level: bool) -> VadConfig {
    let mut cfg = base.clone();
    cfg.provider = "adaptive".to_string();
    cfg.adaptive.noise_margin_db = margin_db;
    cfg.adaptive.use_speech_level = use_speech_level;
    cfg
}

struct Args {
    fixtures: PathBuf,
    models: PathBuf,
    stt_model: PathBuf,
    out: PathBuf,
    vad_min_silence_ms: Option<u32>,
    /// VAD providers to run (`energy`, `adaptive`).
    vad: Vec<String>,
    /// Sweep the adaptive margin and `useSpeechLevel` on the tune scenes and use the pick.
    adaptive_sweep: bool,
    /// Part 3: directory with the denoiser models (`.models/enhance`).
    enhance: Option<PathBuf>,
    /// Part 3: preprocessors to run (default: all of [`ALL_PRE`]).
    pre: Vec<String>,
    /// Part 3: `--pre precomputed:<name>=<dir>` entries.
    precomputed: Vec<(String, PathBuf)>,
    /// Part 3 cost: seconds of S3 audio for the single-stream RTF.
    cost_seconds: usize,
    /// Part 3 cost: seconds each streams-per-core step runs.
    streams_seconds: usize,
    /// Part 1 check lengths in seconds (default [`CHECKS_S`]).
    checks: Vec<f32>,
    /// Part 1 only: also score every check length at this threshold, then stop.
    fixed_threshold: Option<f32>,
    /// With `fixed_threshold`: also the two-window (re-check) FAR on long impostor segments.
    recheck: bool,
    /// With `fixed_threshold`: long other-speech FAR, caller recall and cost per re-check scheme.
    long_other: bool,
    /// With `long_other`: gains of the other speech over the caller start (table C).
    overlap_gains: Vec<GainSpec>,
    /// With `long_other`: overlaps of the other speech over the caller start (table C).
    overlaps: Vec<OverlapSpec>,
    /// Only embedding models whose file stem contains this text.
    model_filter: Option<String>,
}

/// `precomputed:<name>=<dir>` -> `Some((name, dir))`; any other item -> `None`.
fn parse_precomputed(item: &str) -> Result<Option<(String, PathBuf)>, String> {
    let Some(rest) = item.strip_prefix("precomputed:") else {
        return Ok(None);
    };
    match rest.split_once('=') {
        Some((name, dir)) if !name.is_empty() && !dir.is_empty() => {
            Ok(Some((name.to_string(), PathBuf::from(dir))))
        }
        _ => Err(format!(
            "--pre: expected precomputed:<name>=<dir>, got {item}"
        )),
    }
}

fn parse_args() -> Result<Args, String> {
    let (mut fixtures, mut models, mut stt, mut out, mut silence) = (None, None, None, None, None);
    let mut enhance = None;
    let mut vad: Vec<String> = vec!["energy".to_string(), "adaptive".to_string()];
    let mut adaptive_sweep = false;
    let mut pre: Vec<String> = ALL_PRE.iter().map(|p| (*p).to_string()).collect();
    let mut precomputed: Vec<(String, PathBuf)> = Vec::new();
    let (mut cost_seconds, mut streams_seconds) = (60usize, 10usize);
    let mut checks: Vec<f32> = CHECKS_S.to_vec();
    let mut fixed_threshold: Option<f32> = None;
    let mut recheck = false;
    let mut long_other = false;
    let mut overlap_gains: Vec<GainSpec> = vec![GainSpec::Level];
    let mut overlaps: Vec<OverlapSpec> = DEFAULT_OVERLAPS.to_vec();
    let mut model_filter: Option<String> = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--fixtures" => fixtures = Some(PathBuf::from(value("--fixtures")?)),
            "--models" => models = Some(PathBuf::from(value("--models")?)),
            "--stt-model" => stt = Some(PathBuf::from(value("--stt-model")?)),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--vad-min-silence-ms" => {
                silence = Some(
                    value("--vad-min-silence-ms")?
                        .parse::<u32>()
                        .map_err(|e| format!("--vad-min-silence-ms: {e}"))?,
                )
            }
            "--vad" => {
                vad = value("--vad")?
                    .split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect();
                if let Some(bad) = vad
                    .iter()
                    .find(|v| !["energy", "adaptive"].contains(&v.as_str()))
                {
                    return Err(format!("--vad: unknown provider {bad} (energy, adaptive)"));
                }
                if vad.is_empty() {
                    return Err("--vad selects no provider".into());
                }
            }
            "--adaptive-sweep" => adaptive_sweep = true,
            "--enhance" => enhance = Some(PathBuf::from(value("--enhance")?)),
            "--pre" => {
                let all: Vec<String> = value("--pre")?
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
                pre = Vec::new();
                precomputed = Vec::new();
                for item in all {
                    match parse_precomputed(&item)? {
                        Some(entry) => precomputed.push(entry),
                        None => pre.push(item),
                    }
                }
                if let Some(bad) = pre.iter().find(|p| !ALL_PRE.contains(&p.as_str())) {
                    return Err(format!("--pre: unknown preprocessor {bad}"));
                }
            }
            "--cost-seconds" => {
                cost_seconds = value("--cost-seconds")?
                    .parse()
                    .map_err(|e| format!("--cost-seconds: {e}"))?
            }
            "--streams-seconds" => {
                streams_seconds = value("--streams-seconds")?
                    .parse()
                    .map_err(|e| format!("--streams-seconds: {e}"))?
            }
            "--checks" => {
                checks = value("--checks")?
                    .split(',')
                    .map(|c| c.trim().parse::<f32>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("--checks: {e}"))?;
                if checks.is_empty()
                    || checks
                        .iter()
                        .any(|c| *c <= 0.0 || *c * SR as f32 > MAX_CHECK_SAMPLES as f32)
                {
                    return Err("--checks: each length must be > 0 and <= 1.5 s".to_string());
                }
            }
            "--fixed-threshold" => {
                fixed_threshold = Some(
                    value("--fixed-threshold")?
                        .parse()
                        .map_err(|e| format!("--fixed-threshold: {e}"))?,
                )
            }
            "--recheck" => recheck = true,
            "--long-other" => long_other = true,
            "--overlap-gain" => overlap_gains = parse_gains(&value("--overlap-gain")?)?,
            "--overlaps-ms" => overlaps = parse_overlaps(&value("--overlaps-ms")?)?,
            "--model-filter" => model_filter = Some(value("--model-filter")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if long_other && fixed_threshold.is_none() {
        return Err("--long-other requires --fixed-threshold".to_string());
    }
    Ok(Args {
        fixtures: fixtures.ok_or("--fixtures is required")?,
        models: models.ok_or("--models is required")?,
        stt_model: stt.ok_or("--stt-model is required")?,
        out: out.ok_or("--out is required")?,
        vad_min_silence_ms: silence,
        vad,
        adaptive_sweep,
        enhance,
        pre,
        precomputed,
        cost_seconds,
        streams_seconds,
        checks,
        fixed_threshold,
        recheck,
        long_other,
        overlap_gains,
        overlaps,
        model_filter,
    })
}

/// Sample rate of the fixture audio fed to the pipeline (48 kHz stereo PCM), listed in the header.
const BENCH_SAMPLE_RATE: u32 = 48_000;

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let manifest_bytes = fs::read(args.fixtures.join("manifest.json"))
        .map_err(|e| format!("read manifest.json: {e}"))?;
    let manifest_sha = sha256_hex(&manifest_bytes);
    let scenes = load_manifest(&String::from_utf8_lossy(&manifest_bytes))?;
    let mut fx = Fixtures {
        dir: args.fixtures.clone(),
        scenes,
        audio: HashMap::new(),
    };
    fs::create_dir_all(&args.out).map_err(|e| format!("create {}: {e}", args.out.display()))?;

    let mut model_files: Vec<PathBuf> = fs::read_dir(&args.models)
        .map_err(|e| format!("read {}: {e}", args.models.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "onnx"))
        .collect();
    model_files.sort();
    if let Some(filter) = &args.model_filter {
        model_files.retain(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| stem.contains(filter.as_str()))
        });
    }
    if model_files.is_empty() {
        return Err(format!(
            "no .onnx embedding model in {}",
            args.models.display()
        ));
    }

    if args.long_other {
        return long_other_run(&mut fx, &args, &model_files, &manifest_sha);
    }

    // ---- Part 1 ----
    let items = collect_items(&mut fx)?;
    eprintln!(
        "part 1: {} items ({} genuine, {} impostor), {} models",
        items.len(),
        items.iter().filter(|i| i.genuine).count(),
        items.iter().filter(|i| !i.genuine).count(),
        model_files.len()
    );
    let mut embedders = Vec::new();
    let mut p1_rows: Vec<Part1Row> = Vec::new();
    let mut real_rows: Vec<RealRow> = Vec::new();
    let mut ref_cache: HashMap<(usize, usize), Vec<f32>> = HashMap::new(); // (model, scene)
    let mut fixed_md = String::new();
    for (m, path) in model_files.iter().enumerate() {
        let mut emb = Embedder::open(path, args.checks.len())?;
        eprintln!("model {}", emb.id);
        let mut refs_needed: Vec<usize> = items.iter().map(|i| i.ref_scene).collect();
        refs_needed.sort_unstable();
        refs_needed.dedup();
        for scene in refs_needed {
            let enroll = enrollment(&mut fx, scene)?;
            let (e, _) = emb.embed(&enroll).ok_or("enrollment too short to embed")?;
            ref_cache.insert((m, scene), e);
        }
        for (c, check_s) in args.checks.iter().enumerate() {
            let mut scores = Vec::with_capacity(items.len());
            for item in &items {
                let scored = emb.embed(first(&item.clip, check_samples(*check_s)));
                scores.push(scored.map(|(e, secs)| {
                    emb.timings[c].push(secs);
                    cosine(&ref_cache[&(m, item.ref_scene)], &e)
                }));
            }
            if let Some(t) = args.fixed_threshold {
                fixed_md.push_str(&part1_fixed_table(&emb.id, *check_s, t, &items, &scores));
                if args.recheck {
                    let n = check_samples(*check_s);
                    let mut second = Vec::with_capacity(items.len());
                    for item in &items {
                        second.push(if item.genuine || item.clip.len() < 2 * n {
                            None
                        } else {
                            emb.embed(&item.clip[n..2 * n])
                                .map(|(e, _)| cosine(&ref_cache[&(m, item.ref_scene)], &e))
                        });
                    }
                    fixed_md.push_str(&recheck_far_table(
                        &emb.id, *check_s, t, &items, &scores, &second,
                    ));
                }
            }
            p1_rows.extend(part1_rows(&emb.id, *check_s, &items, &scores));
            if let Some(row) = real_voice_row(&emb, &args.models, *check_s)? {
                real_rows.push(row);
            }
        }
        embedders.push(emb);
    }

    if args.fixed_threshold.is_some() {
        let path = args.out.join("part1_fixed.md");
        fs::write(&path, &fixed_md).map_err(|e| format!("write {}: {e}", path.display()))?;
        eprint!("{fixed_md}");
        eprintln!("--fixed-threshold: Part 1 only, wrote {}", path.display());
        return Ok(());
    }

    // ---- Part 2 ----
    // Best (model, check): lowest tune-set EER in row "all".
    let best = p1_rows
        .iter()
        .filter(|r| r.level == "all" && r.eer_tune.is_some() && r.threshold_tune.is_some())
        .min_by(|a, b| a.eer_tune.unwrap().total_cmp(&b.eer_tune.unwrap()))
        .ok_or("part 2: no (model, check length) with both genuine and impostor tune items")?;
    let best_model = embedders
        .iter()
        .position(|e| e.id == best.model)
        .ok_or("best model vanished")?;
    let (best_check, best_threshold) = (best.check_s, best.threshold_tune.unwrap());
    eprintln!(
        "part 2: model {} check {} s threshold {:.4}",
        best.model, best_check, best_threshold
    );

    let s3_test: Vec<usize> = (0..fx.scenes.len())
        .filter(|i| {
            fx.scenes[*i].kind == "conversation"
                && fx.scenes[*i].set == Set::Test
                && fx.scenes[*i].main_db == -20
        })
        .collect();
    let (part2_scenes, part2_set) = if s3_test.is_empty() {
        (
            (0..fx.scenes.len())
                .filter(|i| fx.scenes[*i].kind == "conversation" && fx.scenes[*i].main_db == -20)
                .collect::<Vec<_>>(),
            "tune (no test-set conversation scene in the fixtures: smoke only, threshold not held out)",
        )
    } else {
        (s3_test, "test")
    };

    let phrases_main: HashSet<String> = fx
        .scenes
        .iter()
        .flat_map(|s| s.segments.iter())
        .filter(|s| s.role == "main")
        .flat_map(|s| words_of(&s.text))
        .collect();
    let leak_vocab: HashSet<String> = fx
        .scenes
        .iter()
        .flat_map(|s| s.segments.iter())
        .filter(|s| s.role != "main")
        .flat_map(|s| words_of(&s.text))
        .filter(|w| w.len() >= 4 && !phrases_main.contains(w))
        .collect();

    // The product's default VAD. Silero was evaluated and dropped (it merged every babble scene
    // into one utterance, benchmarks/speaker/README.md).
    let mut vad_config = VadConfig::default();
    if let Some(ms) = args.vad_min_silence_ms {
        vad_config.min_silence_duration_ms = ms;
    }
    let recognizer = create_recognizer(&args.stt_model)?;

    // The adaptive VAD: defaults, or the sweep's pick on the tune scenes (raw audio, AGC off).
    let mut adaptive_cfg = adaptive_vad_config(
        &vad_config,
        vad_config.adaptive.noise_margin_db,
        vad_config.adaptive.use_speech_level,
    );
    let mut sweep_json = String::new();
    let mut sweep_md = String::new();
    if args.adaptive_sweep && args.vad.iter().any(|v| v == "adaptive") {
        let tune_scenes: Vec<usize> = (0..fx.scenes.len())
            .filter(|i| fx.scenes[*i].kind == "conversation" && fx.scenes[*i].set == Set::Tune)
            .collect();
        let sweep_stats =
            |fx: &mut Fixtures, cfg: &VadConfig, name: &str| -> Result<LevelStats, String> {
                let p2 = P2 {
                    enhance: None,
                    emb: &embedders[best_model],
                    check_s: best_check,
                    threshold: best_threshold,
                    recognizer: &recognizer,
                    vad_name: name,
                    vad_config: cfg,
                    leak_vocab: &leak_vocab,
                };
                let mut total = LevelStats::default();
                for &scene in &tune_scenes {
                    eprintln!("sweep {name} scene {}", fx.scenes[scene].id);
                    let (stats, _) = part2_scene(fx, scene, &p2)?;
                    add_stats(&mut total, &stats);
                }
                Ok(total)
            };
        let energy = sweep_stats(&mut fx, &vad_config, "sweep-energy")?;
        let bound = energy.merged + energy.unmatched;
        let mut sweep: Vec<SweepRow> = Vec::new();
        for margin in SWEEP_MARGINS {
            for use_level in [false, true] {
                let cfg = adaptive_vad_config(&vad_config, margin, use_level);
                let st = sweep_stats(&mut fx, &cfg, "sweep-adaptive")?;
                sweep.push(SweepRow {
                    margin_db: margin,
                    use_speech_level: use_level,
                    main_turns: st.main_turns,
                    detected: st.main_turns - st.main_missed,
                    merged: st.merged,
                    unmatched: st.unmatched,
                });
            }
        }
        let pick = pick_sweep(&sweep, bound);
        if let Some(i) = pick {
            adaptive_cfg =
                adaptive_vad_config(&vad_config, sweep[i].margin_db, sweep[i].use_speech_level);
        }
        sweep_md.push_str(&format!(
            "\n## Adaptive VAD sweep (tune scenes: {}, raw audio, AGC off)\n\nEnergy on the same scenes: {} of {} main turns detected, merged + unmatched = {} (the bound). Pick: most detected main turns with merged + unmatched <= the bound; tie: larger margin, then `useSpeechLevel` false.\n\n| noiseMarginDb | useSpeechLevel | main turns detected | merged utt | unmatched utt | within bound | pick |\n|---|---|---|---|---|---|---|\n",
            tune_scenes.len(),
            energy.main_turns - energy.main_missed,
            energy.main_turns,
            bound
        ));
        let mut rows_json = Vec::new();
        for (i, r) in sweep.iter().enumerate() {
            let within = r.merged + r.unmatched <= bound;
            sweep_md.push_str(&format!(
                "| {} | {} | {} / {} | {} | {} | {} | {} |\n",
                r.margin_db,
                r.use_speech_level,
                r.detected,
                r.main_turns,
                r.merged,
                r.unmatched,
                if within { "yes" } else { "no" },
                if pick == Some(i) { "**pick**" } else { "" }
            ));
            rows_json.push(format!(
                "      {{\"noise_margin_db\": {}, \"use_speech_level\": {}, \"main_turns\": {}, \"detected\": {}, \"merged\": {}, \"unmatched\": {}, \"within_bound\": {}}}",
                r.margin_db, r.use_speech_level, r.main_turns, r.detected, r.merged, r.unmatched, within
            ));
        }
        if pick.is_none() {
            sweep_md.push_str(
                "\nNo candidate stayed within the bound: the adaptive defaults are used below.\n",
            );
        }
        sweep_json = format!(
            "  \"adaptive_sweep\": {{\n    \"scenes\": {},\n    \"energy_detected\": {},\n    \"energy_main_turns\": {},\n    \"bound_merged_plus_unmatched\": {},\n    \"pick\": {},\n    \"rows\": [\n{}\n    ]\n  }},\n",
            tune_scenes.len(),
            energy.main_turns - energy.main_missed,
            energy.main_turns,
            bound,
            pick.map_or("null".to_string(), |i| format!(
                "{{\"noise_margin_db\": {}, \"use_speech_level\": {}}}",
                sweep[i].margin_db, sweep[i].use_speech_level
            )),
            rows_json.join(",\n")
        );
    }

    let vad_runs: Vec<(String, VadConfig)> = args
        .vad
        .iter()
        .map(|v| {
            if v == "adaptive" {
                (v.clone(), adaptive_cfg.clone())
            } else {
                (v.clone(), vad_config.clone())
            }
        })
        .collect();
    let mut outcomes: Vec<VariantOutcome> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    for (name, cfg) in &vad_runs {
        let p2 = P2 {
            enhance: None,
            emb: &embedders[best_model],
            check_s: best_check,
            threshold: best_threshold,
            recognizer: &recognizer,
            vad_name: name,
            vad_config: cfg,
            leak_vocab: &leak_vocab,
        };
        let mut by_level: BTreeMap<i32, LevelStats> = BTreeMap::new();
        for &scene in &part2_scenes {
            let level = fx.scenes[scene].level_db;
            eprintln!("part 2 vad {name} scene {}", fx.scenes[scene].id);
            let (stats, scene_rows) = part2_scene(&mut fx, scene, &p2)?;
            add_stats(by_level.entry(level).or_default(), &stats);
            rows.extend(scene_rows);
        }
        outcomes.push(VariantOutcome {
            name: name.clone(),
            min_silence_ms: cfg.min_silence_duration_ms,
            pre_roll_ms: pre_roll_ms(cfg),
            levels: by_level,
        });
    }

    // ---- Part 3 ----
    let part3 = match &args.enhance {
        None => None,
        Some(models) => {
            // Same speaker model and threshold as Grid A needs: TitaNet-small at 1.5 s when
            // present, else the Part 2 pick.
            let titanet = p1_rows.iter().find(|r| {
                r.level == "all"
                    && r.check_s == 1.5
                    && r.model.contains("titanet")
                    && r.threshold_tune.is_some()
            });
            let (model_idx, check_s, threshold) = match titanet {
                Some(r) => (
                    embedders
                        .iter()
                        .position(|e| e.id == r.model)
                        .ok_or("titanet model vanished")?,
                    r.check_s,
                    r.threshold_tune.unwrap(),
                ),
                None => (best_model, best_check, best_threshold),
            };
            let mut runs = Vec::new();
            for (i, (name, cfg)) in vad_runs.iter().enumerate() {
                let run3 = Part3Run {
                    models,
                    pre_filter: &args.pre,
                    precomputed: &args.precomputed,
                    emb: &embedders[model_idx],
                    check_s,
                    threshold,
                    recognizer: &recognizer,
                    vad_name: name,
                    vad_config: cfg,
                    leak_vocab: &leak_vocab,
                    with_cost: i == 0,
                    cost_seconds: args.cost_seconds,
                    streams_seconds: args.streams_seconds,
                };
                runs.push(part3_run(&mut fx, &run3, &mut rows)?);
            }
            Some(runs)
        }
    };
    let turns_jsonl: String = rows.iter().map(|r| row_json(r) + "\n").collect();
    fs::write(args.out.join("turns.jsonl"), turns_jsonl)
        .map_err(|e| format!("write turns.jsonl: {e}"))?;
    fs::write(args.out.join("conversations.md"), conversations_md(&rows))
        .map_err(|e| format!("write conversations.md: {e}"))?;

    // ---- Output ----
    let nproc = std::thread::available_parallelism().map_or(0, |n| n.get());
    let cpu = cpu_model();
    let sha = git_sha();

    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(&format!("  \"nproc\": {nproc},\n"));
    json.push_str(&format!("  \"cpu_model\": \"{}\",\n", json_escape(&cpu)));
    json.push_str(&format!("  \"git_sha\": \"{}\",\n", json_escape(&sha)));
    json.push_str(&format!("  \"manifest_sha256\": \"{manifest_sha}\",\n"));
    json.push_str(&format!("  \"sample_rate\": {BENCH_SAMPLE_RATE},\n"));
    json.push_str("  \"part1\": {\n    \"rows\": [\n");
    for (i, r) in p1_rows.iter().enumerate() {
        json.push_str(&format!(
            "      {{\"model\": \"{}\", \"check_s\": {}, \"level\": \"{}\", \"n_genuine_tune\": {}, \"n_impostor_tune\": {}, \"eer_tune\": {}, \"frr_at_far1_tune\": {}, \"threshold_tune\": {}, \"n_genuine_test\": {}, \"n_impostor_test\": {}, \"frr_test\": {}, \"far_test\": {}}}{}\n",
            json_escape(&r.model), r.check_s, r.level, r.n_gen_tune, r.n_imp_tune,
            json_num(r.eer_tune), json_num(r.frr_at_far1_tune),
            json_num(r.threshold_tune.map(f64::from)), r.n_gen_test, r.n_imp_test,
            json_num(r.frr_test), json_num(r.far_test),
            if i + 1 < p1_rows.len() { "," } else { "" }
        ));
    }
    json.push_str("    ],\n    \"real_voice\": [\n");
    for (i, r) in real_rows.iter().enumerate() {
        json.push_str(&format!(
            "      {{\"model\": \"{}\", \"check_s\": {}, \"n_genuine\": {}, \"n_impostor\": {}, \"eer\": {}, \"frr_at_far1\": {}, \"threshold\": {}}}{}\n",
            json_escape(&r.model), r.check_s, r.n_gen, r.n_imp, json_num(r.eer),
            json_num(r.frr_at_far1), json_num(r.threshold.map(f64::from)),
            if i + 1 < real_rows.len() { "," } else { "" }
        ));
    }
    json.push_str("    ],\n    \"compute_ms\": [\n");
    let mut timing_lines = Vec::new();
    for emb in &embedders {
        for (c, check_s) in args.checks.iter().enumerate() {
            let mut ms: Vec<f64> = emb.timings[c].iter().map(|s| s * 1000.0).collect();
            ms.sort_by(|a, b| a.total_cmp(b));
            timing_lines.push((
                emb.id.clone(),
                *check_s,
                percentile(&ms, 0.5),
                percentile(&ms, 0.99),
                ms.len(),
            ));
        }
    }
    for (i, (model, check_s, p50, p99, n)) in timing_lines.iter().enumerate() {
        json.push_str(&format!(
            "      {{\"model\": \"{}\", \"check_s\": {check_s}, \"p50\": {:.3}, \"p99\": {:.3}, \"n\": {n}}}{}\n",
            json_escape(model), p50, p99,
            if i + 1 < timing_lines.len() { "," } else { "" }
        ));
    }
    json.push_str("    ]\n  },\n");
    json.push_str(&sweep_json);
    json.push_str(&format!(
        "  \"part2\": {{\n    \"model\": \"{}\",\n    \"check_s\": {},\n    \"threshold\": {:.6},\n    \"set\": \"{}\",\n    \"vad\": {{\n",
        json_escape(&embedders[best_model].id), best_check, best_threshold, json_escape(part2_set),
    ));
    for (v, o) in outcomes.iter().enumerate() {
        let comma = if v + 1 < outcomes.len() { "," } else { "" };
        {
            {
                let by_level = &o.levels;
                json.push_str(&format!(
                    "      \"{}\": {{\n        \"vad_min_silence_ms\": {},\n        \"pre_roll_ms\": {},\n        \"levels\": [\n",
                    o.name, o.min_silence_ms, o.pre_roll_ms
                ));
                let level_count = by_level.len();
                for (i, (level, s)) in by_level.iter().enumerate() {
                    json.push_str(&format!(
                        "          {{\"level_db\": {level}, \"vad_utterances\": {}, \"merged_utterances\": {}, \"main_turns\": {}, \"main_missed\": {}, \"qualified_turns\": {}, \"a_qualified_oracle_pct\": {}, \"long_turns\": {}, \"long_wer_pct\": {}, \"long_word_similarity_pct\": {}, \"b_main_caller_pct\": {}, \"b_qualified_caller_pct\": {}, \"b_long_caller_pct\": {}, \"bystander_turns\": {}, \"bystander_missed\": {}, \"c_bystander_other_pct\": {}, \"d_bystander_caller_pct\": {}, \"e_leakage_pct\": {}, \"f_unmatched_utterances\": {}, \"f_unmatched_caller\": {}, \"f_unmatched_other\": {}, \"median_onset_ms\": {}}}{}\n",
                        s.utterances, s.merged, s.main_turns, s.main_missed,
                        s.q_turns,
                        json_num(pct(s.q_recognized, s.q_turns)),
                        s.l_turns,
                        json_num(pct(s.l_edits, s.l_ref_words)),
                        json_num((s.l_turns > 0).then(|| 100.0 * s.l_sim_sum / s.l_turns as f64)),
                        json_num(pct(s.main_caller, s.main_turns)),
                        json_num(pct(s.q_caller, s.q_turns)),
                        json_num(pct(s.l_caller, s.l_turns)),
                        s.byst_turns, s.byst_missed,
                        json_num(pct(s.byst_other, s.byst_turns)),
                        json_num(pct(s.byst_caller, s.byst_turns)),
                        json_num(pct(s.leak_words, s.main_words)),
                        s.unmatched, s.unmatched_caller, s.unmatched_other,
                        json_num(median_i64(&s.onset_delays)),
                        if i + 1 < level_count { "," } else { "" }
                    ));
                }
                json.push_str(&format!("        ]\n      }}{comma}\n"));
            }
        }
    }
    json.push_str("    }\n  }");
    if let Some(runs) = &part3 {
        for (i, p3) in runs.iter().enumerate() {
            json.push_str(",\n");
            let key = if i == 0 {
                "part3".to_string()
            } else {
                format!("part3_{}", p3.vad)
            };
            json.push_str(&part3_json(p3, &key));
        }
    }
    json.push_str("\n}\n");
    fs::write(args.out.join("results.json"), json)
        .map_err(|e| format!("write results.json: {e}"))?;

    let mut md = String::new();
    md.push_str("# Speaker-ID bench\n\n");
    md.push_str(&format!(
        "nproc {nproc}, CPU {cpu}, git {sha}, manifest sha256 `{manifest_sha}`, sample_rate {BENCH_SAMPLE_RATE} Hz\n\n"
    ));
    md.push_str("## Part 1: embedding level\n\n");
    md.push_str("Threshold, EER and FRR@FAR1% come from the tune set; the last two columns apply that threshold to the test set. Accept = score above threshold.\n\n");
    for emb in &embedders {
        for &check_s in &args.checks {
            md.push_str(&format!("### {} / {} s\n\n", emb.id, check_s));
            md.push_str("| level | gen/imp tune | EER tune | FRR@FAR1% tune | threshold | gen/imp test | FRR test | FAR test |\n|---|---|---|---|---|---|---|---|\n");
            for r in p1_rows
                .iter()
                .filter(|r| r.model == emb.id && r.check_s == check_s)
            {
                md.push_str(&format!(
                    "| {} | {}/{} | {} | {} | {} | {}/{} | {} | {} |\n",
                    r.level,
                    r.n_gen_tune,
                    r.n_imp_tune,
                    fmt_opt(r.eer_tune, 3),
                    fmt_opt(r.frr_at_far1_tune, 3),
                    fmt_opt(r.threshold_tune.map(f64::from), 3),
                    r.n_gen_test,
                    r.n_imp_test,
                    fmt_opt(r.frr_test, 3),
                    fmt_opt(r.far_test, 3)
                ));
            }
            if let Some(r) = real_rows
                .iter()
                .find(|r| r.model == emb.id && r.check_s == check_s)
            {
                md.push_str(&format!(
                    "| real voices (clean) | {}/{} | {} | {} | {} | - | - | - |\n",
                    r.n_gen,
                    r.n_imp,
                    fmt_opt(r.eer, 3),
                    fmt_opt(r.frr_at_far1, 3),
                    fmt_opt(r.threshold.map(f64::from), 3)
                ));
            }
            md.push('\n');
        }
    }
    md.push_str("### Embedding compute time (one core, ms per clip)\n\n| model | check | p50 | p99 | clips |\n|---|---|---|---|---|\n");
    for (model, check_s, p50, p99, n) in &timing_lines {
        md.push_str(&format!(
            "| {model} | {check_s} s | {p50:.1} | {p99:.1} | {n} |\n"
        ));
    }
    md.push_str(&format!(
        "\n## Part 2: end to end\n\nModel `{}`, check {} s, threshold {:.4} (tune set, FAR 1 %). Scenes: {}. STT input and embedding clip start at SpeechStart minus the product pre-roll (`speech_pad_ms + min_speech_duration_ms`). Qualified phrases are judged with the 0.75 oracle; long sentences are measured (WER, not qualified). `turns.jsonl` and `conversations.md` list every turn.\n",
        embedders[best_model].id, best_check, best_threshold, part2_set
    ));
    for o in &outcomes {
        {
            {
                let by_level = &o.levels;
                md.push_str(&format!(
                    "\n### VAD {} (min_silence {} ms, pre-roll {} ms)\n\n",
                    o.name, o.min_silence_ms, o.pre_roll_ms
                ));
                md.push_str("| level | VAD utt | merged utt | main turns (missed) | (a) oracle % (qualified) | WER % (long, measured, not qualified) | (b) main to caller %: all / qualified / long | bystander turns (missed) | (c) other % | (d) caller % | (e) leakage % | (f) unmatched utt (caller/other) | median onset ms |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
                for (level, s) in by_level {
                    md.push_str(&format!(
                        "| {level} dB | {} | {} | {} ({}) | {} | {} | {} / {} / {} | {} ({}) | {} | {} | {} | {} ({}/{}) | {} |\n",
                        s.utterances,
                        s.merged,
                        s.main_turns,
                        s.main_missed,
                        fmt_opt(pct(s.q_recognized, s.q_turns), 1),
                        fmt_opt(pct(s.l_edits, s.l_ref_words), 1),
                        fmt_opt(pct(s.main_caller, s.main_turns), 1),
                        fmt_opt(pct(s.q_caller, s.q_turns), 1),
                        fmt_opt(pct(s.l_caller, s.l_turns), 1),
                        s.byst_turns,
                        s.byst_missed,
                        fmt_opt(pct(s.byst_other, s.byst_turns), 1),
                        fmt_opt(pct(s.byst_caller, s.byst_turns), 1),
                        fmt_opt(pct(s.leak_words, s.main_words), 2),
                        s.unmatched,
                        s.unmatched_caller,
                        s.unmatched_other,
                        fmt_opt(median_i64(&s.onset_delays), 0)
                    ));
                }
            }
        }
    }
    md.push_str(&sweep_md);
    if let Some(runs) = &part3 {
        for p3 in runs {
            md.push_str(&part3_md(p3));
        }
    }
    fs::write(args.out.join("summary.md"), md).map_err(|e| format!("write summary.md: {e}"))?;
    eprintln!("wrote {}/results.json and summary.md", args.out.display());
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// --long-other: re-check schemes on long other-speech streams
// ---------------------------------------------------------------------------------------------

/// Product pre-roll in front of a scored window (300 ms at 16 kHz).
const PRE_ROLL_SAMPLES: usize = 4_800;
/// Hop of the sliding scheme (500 ms).
const SLIDING_HOP: usize = 8_000;
/// Bystander unions stitched into one long other-speech stream.
const STREAM_UNIONS: usize = 4;
/// Exposure times of table B, seconds.
const EXPOSURES_S: [f32; 8] = [1.5, 3.0, 4.5, 6.0, 7.5, 9.0, 12.0, 15.0];
/// Seconds of other speech before the caller starts (table C).
const RECALL_K_S: [f32; 6] = [1.0, 2.0, 3.0, 4.0, 6.0, 9.0];
/// Default overlaps (`--overlaps-ms`): ms the other speech runs over the start of the caller.
const DEFAULT_OVERLAPS: [OverlapSpec; 2] = [OverlapSpec::Ms(0), OverlapSpec::Ms(1000)];

/// How long the other speech runs over the start of the caller (table C).
#[derive(Clone, Copy, Debug, PartialEq)]
enum OverlapSpec {
    Ms(usize),
    /// The whole caller clip.
    Full,
}

impl OverlapSpec {
    fn samples(self, clip_len: usize) -> usize {
        match self {
            OverlapSpec::Ms(ms) => ms * 16,
            OverlapSpec::Full => clip_len,
        }
    }

    fn label(self) -> String {
        match self {
            OverlapSpec::Ms(ms) => ms.to_string(),
            OverlapSpec::Full => "full".to_string(),
        }
    }

    fn json(self) -> String {
        match self {
            OverlapSpec::Ms(ms) => ms.to_string(),
            OverlapSpec::Full => "\"full\"".to_string(),
        }
    }
}

/// Gain of the other speech in the overlap (table C).
#[derive(Clone, Copy, Debug, PartialEq)]
enum GainSpec {
    /// 10^(level_db / 20) of the caller clip's scene.
    Level,
    Linear(f32),
}

impl GainSpec {
    fn resolve(self, level_db: i32) -> f32 {
        match self {
            GainSpec::Level => 10f32.powf(level_db as f32 / 20.0),
            GainSpec::Linear(g) => g,
        }
    }

    fn label(self) -> String {
        match self {
            GainSpec::Level => "level".to_string(),
            GainSpec::Linear(g) => format!("{g}"),
        }
    }

    fn json(self) -> String {
        match self {
            GainSpec::Level => "\"level\"".to_string(),
            GainSpec::Linear(g) => format!("{g}"),
        }
    }
}

fn parse_overlaps(list: &str) -> Result<Vec<OverlapSpec>, String> {
    let out = list
        .split(',')
        .map(|t| match t.trim() {
            "full" => Ok(OverlapSpec::Full),
            t => t
                .parse::<usize>()
                .map(OverlapSpec::Ms)
                .map_err(|e| format!("--overlaps-ms: {t}: {e}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if out.is_empty() {
        return Err("--overlaps-ms: empty list".into());
    }
    Ok(out)
}

fn parse_gains(list: &str) -> Result<Vec<GainSpec>, String> {
    let out = list
        .split(',')
        .map(|t| match t.trim() {
            "level" => Ok(GainSpec::Level),
            t => match t.parse::<f32>() {
                Ok(g) if g.is_finite() && g >= 0.0 => Ok(GainSpec::Linear(g)),
                Ok(_) => Err(format!("--overlap-gain: {t} must be finite and >= 0")),
                Err(e) => Err(format!("--overlap-gain: {t}: {e}")),
            },
        })
        .collect::<Result<Vec<_>, _>>()?;
    if out.is_empty() {
        return Err("--overlap-gain: empty list".into());
    }
    Ok(out)
}

/// Table C input: other[0..onset] ++ clip, with other[onset..] x gain added over the first
/// `min(overlap, clip.len(), other.len() - onset)` samples of the clip.
fn build_recall_input(
    other: &[f32],
    clip: &[f32],
    onset: usize,
    overlap: usize,
    gain: f32,
) -> Vec<f32> {
    let mut x: Vec<f32> = other[..onset].to_vec();
    x.extend_from_slice(clip);
    for i in 0..mixed_samples(other.len(), clip.len(), onset, overlap) {
        x[onset + i] += other[onset + i] * gain;
    }
    x
}

/// Samples of the overlap that [`build_recall_input`] actually mixes.
fn mixed_samples(other_len: usize, clip_len: usize, onset: usize, overlap: usize) -> usize {
    overlap.min(clip_len).min(other_len.saturating_sub(onset))
}
/// Window caps of table C; `None` is unlimited.
const RECALL_CAPS: [Option<usize>; 6] = [Some(2), Some(3), Some(4), Some(6), Some(8), None];
/// Window caps of table D.
const COST_CAPS: [Option<usize>; 4] = [Some(2), Some(4), Some(8), None];
/// Utterance length priced in table D, seconds.
const COST_UTTERANCE_S: usize = 17;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scheme {
    /// Windows end at check, 2 check, 3 check, ... (the product's first check and re-checks).
    Consecutive,
    /// Windows end at check, check + 0.5 s, check + 1 s, ...
    Sliding500,
}

const SCHEMES: [Scheme; 2] = [Scheme::Consecutive, Scheme::Sliding500];

impl Scheme {
    fn name(self) -> &'static str {
        match self {
            Scheme::Consecutive => "consecutive",
            Scheme::Sliding500 => "sliding-500",
        }
    }

    fn step(self, check: usize) -> usize {
        match self {
            Scheme::Consecutive => check,
            Scheme::Sliding500 => SLIDING_HOP,
        }
    }
}

/// Sample positions where windows end for audio of `len` samples.
fn window_ends(scheme: Scheme, len: usize, check: usize) -> Vec<usize> {
    let step = scheme.step(check).max(1);
    let mut ends = Vec::new();
    let mut end = check;
    while end > 0 && end <= len {
        ends.push(end);
        end += step;
    }
    ends
}

/// The first `cap` windows (all when `cap` is `None`).
fn capped(ends: &[usize], cap: Option<usize>) -> &[usize] {
    &ends[..cap.map_or(ends.len(), |n| n.min(ends.len()))]
}

/// Sample range scored for a window ending at `end`: `check` plus the pre-roll, clamped at 0.
fn window_range(end: usize, check: usize) -> (usize, usize) {
    (end.saturating_sub(check + PRE_ROLL_SAMPLES), end)
}

/// Union of millisecond spans, sorted, overlapping and touching spans merged.
fn merge_spans(spans: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut sorted: Vec<(usize, usize)> = spans.iter().copied().filter(|(a, b)| b > a).collect();
    sorted.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in sorted {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// The speech of a scene: the union of its spans cut out of `audio` (16 kHz) and concatenated in
/// time order, gaps removed.
fn speech_union(audio: &[f32], spans_ms: &[(usize, usize)]) -> Vec<f32> {
    let mut out = Vec::new();
    for (a, b) in merge_spans(spans_ms) {
        let from = (a * 16).min(audio.len());
        let to = (b * 16).min(audio.len()).max(from);
        out.extend_from_slice(&audio[from..to]);
    }
    out
}

/// `count` unions starting at `start`, indices modulo the number of unions.
fn stitch(unions: &[Vec<f32>], start: usize, count: usize) -> Vec<f32> {
    (0..count)
        .flat_map(|d| unions[(start + d) % unions.len()].iter().copied())
        .collect()
}

/// End of the first accepting window.
fn first_accept_end(ends: &[usize], accepts: &[bool]) -> Option<usize> {
    ends.iter()
        .zip(accepts)
        .find(|(_, accepted)| **accepted)
        .map(|(end, _)| *end)
}

#[derive(Clone, Copy)]
struct StreamTrial {
    /// Stream length, samples.
    len: usize,
    /// End of the first accepting window over the whole stream.
    first_accept: Option<usize>,
}

/// `(trials, accepted)` at exposure `t` samples: only trials whose stream is at least `t` long
/// count, and a trial is accepted when a window ending at or before `t` accepted.
fn far_at(trials: &[StreamTrial], t: usize) -> (usize, usize) {
    let (mut n, mut hit) = (0, 0);
    for trial in trials.iter().filter(|tr| tr.len >= t) {
        n += 1;
        hit += usize::from(trial.first_accept.is_some_and(|end| end <= t));
    }
    (n, hit)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    /// A window that ended at or before the caller's onset accepted first.
    EarlyFa,
    /// The first accepting window ended after the onset; payload = end - onset, samples.
    Caller(usize),
    /// No window within the cap accepted.
    Miss,
}

/// Outcome of a trial when only the first `cap` windows are scored.
fn classify(ends: &[usize], accepts: &[bool], onset: usize, cap: Option<usize>) -> Outcome {
    let ends = capped(ends, cap);
    match first_accept_end(ends, &accepts[..ends.len().min(accepts.len())]) {
        Some(end) if end <= onset => Outcome::EarlyFa,
        Some(end) => Outcome::Caller(end - onset),
        None => Outcome::Miss,
    }
}

/// Windows scored for an utterance of `len` samples under `cap`.
fn windows_scored(scheme: Scheme, len: usize, check: usize, cap: Option<usize>) -> usize {
    capped(&window_ends(scheme, len, check), cap).len()
}

struct TrialScores {
    onset: usize,
    ends: Vec<usize>,
    accepts: Vec<bool>,
}

struct RecallRun {
    scheme: Scheme,
    gain: GainSpec,
    overlap: OverlapSpec,
    k_s: f32,
    trials: Vec<TrialScores>,
    /// Samples of other speech actually mixed, summed over the trials of one scheme.
    mixed_samples: usize,
}

struct RecallCell {
    n: usize,
    caller: usize,
    early: usize,
    miss: usize,
    lat_p50_ms: Option<f64>,
    lat_p90_ms: Option<f64>,
}

fn recall_cell(trials: &[TrialScores], cap: Option<usize>) -> RecallCell {
    let mut cell = RecallCell {
        n: trials.len(),
        caller: 0,
        early: 0,
        miss: 0,
        lat_p50_ms: None,
        lat_p90_ms: None,
    };
    let mut latencies = Vec::new();
    for t in trials {
        match classify(&t.ends, &t.accepts, t.onset, cap) {
            Outcome::EarlyFa => cell.early += 1,
            Outcome::Miss => cell.miss += 1,
            Outcome::Caller(samples) => {
                cell.caller += 1;
                latencies.push(samples as f64 / 16.0);
            }
        }
    }
    latencies.sort_by(|a, b| a.total_cmp(b));
    if !latencies.is_empty() {
        cell.lat_p50_ms = Some(percentile(&latencies, 0.5));
        cell.lat_p90_ms = Some(percentile(&latencies, 0.9));
    }
    cell
}

struct SetData {
    name: &'static str,
    /// `(level_db of the conversation scene, reference embedding)`.
    refs: Vec<(i32, Vec<f32>)>,
    /// Speech union of every `bystander_alone` scene, manifest order.
    unions: Vec<Vec<f32>>,
    /// Stitched streams, one per union.
    streams: Vec<Vec<f32>>,
}

type EmbedMemo = HashMap<usize, Option<Vec<f32>>>;

fn embed_window(
    emb: &Embedder,
    audio: &[f32],
    end: usize,
    check: usize,
    timings: &mut Vec<f64>,
) -> Option<Vec<f32>> {
    let (from, to) = window_range(end, check);
    let (e, secs) = emb.embed(&audio[from..to])?;
    timings.push(secs);
    Some(e)
}

fn cached_window(
    memo: &mut EmbedMemo,
    emb: &Embedder,
    audio: &[f32],
    end: usize,
    check: usize,
    timings: &mut Vec<f64>,
) -> Option<Vec<f32>> {
    memo.entry(end)
        .or_insert_with(|| embed_window(emb, audio, end, check, timings))
        .clone()
}

fn load_set_data(
    fx: &mut Fixtures,
    emb: &Embedder,
    set: Set,
    name: &'static str,
) -> Result<SetData, String> {
    let ref_scenes: Vec<usize> = (0..fx.scenes.len())
        .filter(|&i| {
            let s = &fx.scenes[i];
            s.kind == "conversation" && s.main_db == -20 && s.set == set
        })
        .collect();
    let mut refs = Vec::new();
    for scene in ref_scenes {
        let enroll = enrollment(fx, scene)?;
        let (e, _) = emb.embed(&enroll).ok_or("enrollment too short to embed")?;
        refs.push((fx.scenes[scene].level_db, e));
    }
    let bystander_scenes: Vec<usize> = (0..fx.scenes.len())
        .filter(|&i| fx.scenes[i].kind == "bystander_alone" && fx.scenes[i].set == set)
        .collect();
    let mut unions = Vec::new();
    for scene in bystander_scenes {
        let spans: Vec<(usize, usize)> = fx.scenes[scene]
            .segments
            .iter()
            .map(|s| (s.start_ms, s.end_ms))
            .collect();
        let audio = fx.audio16(scene)?.clone();
        unions.push(speech_union(&audio, &spans));
    }
    if refs.is_empty() || unions.is_empty() {
        return Err(format!(
            "--long-other: set {name} needs conversation scenes at -20 dB and bystander_alone scenes"
        ));
    }
    let streams = (0..unions.len())
        .map(|i| stitch(&unions, i, STREAM_UNIONS))
        .collect();
    Ok(SetData {
        name,
        refs,
        unions,
        streams,
    })
}

struct CallerClip {
    audio: Vec<f32>,
    level_db: i32,
}

/// `main` segments of the test set's default-level `main_over_babble` scenes, filtered like
/// `collect_items`.
fn caller_clips(fx: &mut Fixtures) -> Result<Vec<CallerClip>, String> {
    let mut out = Vec::new();
    for scene in 0..fx.scenes.len() {
        let s = &fx.scenes[scene];
        if s.kind != "main_over_babble"
            || s.set != Set::Test
            || !default_level_scene(&s.kind, s.main_db)
        {
            continue;
        }
        let level_db = s.level_db;
        let ref_scene = fx
            .reference_scene(Set::Test, Some(level_db))
            .ok_or("fixtures have no conversation scene (needed for the enrollment reference)")?;
        let enroll_texts: Vec<String> = fx.scenes[ref_scene]
            .segments
            .iter()
            .filter(|s| s.enroll)
            .map(|s| s.text.clone())
            .collect();
        let spans: Vec<(usize, usize)> = fx.scenes[scene]
            .segments
            .iter()
            .filter(|s| s.role == "main" && !shares_enrollment_text(&s.text, &enroll_texts))
            .map(|s| (s.start_ms, s.end_ms))
            .collect();
        if spans.is_empty() {
            continue;
        }
        let audio = fx.audio16(scene)?;
        for (start_ms, end_ms) in spans {
            let from = (start_ms * 16).min(audio.len());
            let to = (end_ms * 16).min(audio.len()).max(from);
            out.push(CallerClip {
                audio: audio[from..to].to_vec(),
                level_db,
            });
        }
    }
    Ok(out)
}

/// Table A: `(windows scored, accepted)` over consecutive windows of every union, per reference.
fn marginal_far(
    emb: &Embedder,
    d: &SetData,
    check: usize,
    threshold: f32,
    timings: &mut Vec<f64>,
) -> (usize, usize) {
    let (mut n, mut hit) = (0, 0);
    for union in &d.unions {
        for end in window_ends(Scheme::Consecutive, union.len(), check) {
            let Some(e) = embed_window(emb, union, end, check, timings) else {
                continue;
            };
            for (_, r) in &d.refs {
                n += 1;
                hit += usize::from(cosine(r, &e) >= threshold);
            }
        }
    }
    (n, hit)
}

/// Table B trials of one scheme: one per (stream, reference).
fn stream_trials(
    emb: &Embedder,
    d: &SetData,
    scheme: Scheme,
    check: usize,
    threshold: f32,
    memos: &mut [EmbedMemo],
    timings: &mut Vec<f64>,
) -> Vec<StreamTrial> {
    let mut trials = Vec::new();
    for (i, stream) in d.streams.iter().enumerate() {
        let ends = window_ends(scheme, stream.len(), check);
        let embs: Vec<Option<Vec<f32>>> = ends
            .iter()
            .map(|&end| cached_window(&mut memos[i], emb, stream, end, check, timings))
            .collect();
        for (_, r) in &d.refs {
            let accepts: Vec<bool> = embs
                .iter()
                .map(|e| e.as_ref().is_some_and(|e| cosine(r, e) >= threshold))
                .collect();
            trials.push(StreamTrial {
                len: stream.len(),
                first_accept: first_accept_end(&ends, &accepts),
            });
        }
    }
    trials
}

/// Table C: every (scheme, overlap, k) with one trial per caller clip.
#[allow(clippy::too_many_arguments)]
fn caller_recall(
    emb: &Embedder,
    test: &SetData,
    clips: &[CallerClip],
    check: usize,
    threshold: f32,
    gains: &[GainSpec],
    overlaps: &[OverlapSpec],
    memos: &mut [EmbedMemo],
    timings: &mut Vec<f64>,
) -> Vec<RecallRun> {
    let mut runs: Vec<RecallRun> = Vec::new();
    for scheme in SCHEMES {
        for gain in gains {
            for overlap in overlaps {
                for k_s in RECALL_K_S {
                    runs.push(RecallRun {
                        scheme,
                        gain: *gain,
                        overlap: *overlap,
                        k_s,
                        trials: Vec::new(),
                        mixed_samples: 0,
                    });
                }
            }
        }
    }
    let run_index = |si: usize, gi: usize, oi: usize, ki: usize| {
        ((si * gains.len() + gi) * overlaps.len() + oi) * RECALL_K_S.len() + ki
    };
    for (j, clip) in clips.iter().enumerate() {
        let sidx = j % test.streams.len();
        let other = &test.streams[sidx];
        let reference = test
            .refs
            .iter()
            .find(|(level, _)| *level == clip.level_db)
            .unwrap_or(&test.refs[0])
            .1
            .as_slice();
        for (gi, gain_spec) in gains.iter().enumerate() {
            let gain = gain_spec.resolve(clip.level_db);
            for (oi, overlap_spec) in overlaps.iter().enumerate() {
                let overlap = overlap_spec.samples(clip.audio.len());
                for (ki, k_s) in RECALL_K_S.iter().enumerate() {
                    let onset = ((k_s * SR as f32) as usize).min(other.len());
                    let x = build_recall_input(other, &clip.audio, onset, overlap, gain);
                    let mixed = mixed_samples(other.len(), clip.audio.len(), onset, overlap);
                    let mut accepted_at: HashMap<usize, bool> = HashMap::new();
                    for (si, scheme) in SCHEMES.iter().enumerate() {
                        let ends = window_ends(*scheme, x.len(), check);
                        let mut accepts = Vec::with_capacity(ends.len());
                        for &end in &ends {
                            let accepted = *accepted_at.entry(end).or_insert_with(|| {
                                // Before the onset the window is pure other speech: shared with
                                // table B's stream cache.
                                let e = if end <= onset {
                                    cached_window(&mut memos[sidx], emb, other, end, check, timings)
                                } else {
                                    embed_window(emb, &x, end, check, timings)
                                };
                                e.is_some_and(|e| cosine(reference, &e) >= threshold)
                            });
                            accepts.push(accepted);
                        }
                        let run = &mut runs[run_index(si, gi, oi, ki)];
                        run.mixed_samples += mixed;
                        run.trials.push(TrialScores {
                            onset,
                            ends,
                            accepts,
                        });
                    }
                }
            }
        }
    }
    runs
}

fn cap_label(cap: Option<usize>) -> String {
    cap.map_or("unlimited".to_string(), |n| n.to_string())
}

fn cap_json(cap: Option<usize>) -> String {
    cap.map_or("null".to_string(), |n| n.to_string())
}

/// Tables A-D for one (model, check length) as `(markdown, json object)`.
fn long_other_section(
    fx: &mut Fixtures,
    emb: &Embedder,
    check_s: f32,
    threshold: f32,
    gains: &[GainSpec],
    overlaps: &[OverlapSpec],
) -> Result<(String, String), String> {
    let check = check_samples(check_s);
    let sets = [
        load_set_data(fx, emb, Set::Tune, "tune")?,
        load_set_data(fx, emb, Set::Test, "test")?,
    ];
    let test = &sets[1];
    let clips = caller_clips(fx)?;
    if clips.is_empty() {
        return Err("--long-other: no caller clips in the test set".into());
    }
    let mut timings: Vec<f64> = Vec::new();
    let mut memos: Vec<Vec<EmbedMemo>> = sets
        .iter()
        .map(|d| vec![EmbedMemo::new(); d.streams.len()])
        .collect();

    let mut md = format!(
        "## {} / check {check_s} s\n\nReferences: {}. Streams per set: tune {}, test {}. Caller clips: {}.\n\n",
        emb.id,
        sets.iter()
            .map(|d| format!("{} n={}", d.name, d.refs.len()))
            .collect::<Vec<_>>()
            .join(", "),
        sets[0].streams.len(),
        sets[1].streams.len(),
        clips.len()
    );
    let mut json = format!(
        "{{\"model\": \"{}\", \"check_s\": {check_s}, \"refs\": {{{}}}, \"streams\": {{{}}}, \"caller_clips\": {}",
        json_escape(&emb.id),
        sets.iter()
            .map(|d| format!("\"{}\": {}", d.name, d.refs.len()))
            .collect::<Vec<_>>()
            .join(", "),
        sets.iter()
            .map(|d| format!("\"{}\": {}", d.name, d.streams.len()))
            .collect::<Vec<_>>()
            .join(", "),
        clips.len()
    );

    // ---- A ----
    md.push_str("### A) Marginal FAR per consecutive window\n\nWindows of each `bystander_alone` speech union (window ends <= union length), pooled over references; accept = score >= threshold.\n\n| set | refs | windows x refs | accepted | FAR % |\n|---|---|---|---|---|\n");
    let mut a_rows = Vec::new();
    let (mut pooled_n, mut pooled_hit) = (0, 0);
    let mut p_by_set = Vec::new();
    for d in &sets {
        let (n, hit) = marginal_far(emb, d, check, threshold, &mut timings);
        pooled_n += n;
        pooled_hit += hit;
        p_by_set.push(hit as f64 / n.max(1) as f64);
        md.push_str(&format!(
            "| {} | {} | {n} | {hit} | {} |\n",
            d.name,
            d.refs.len(),
            fmt_opt(pct(hit, n), 2)
        ));
        a_rows.push(format!(
            "{{\"set\": \"{}\", \"refs\": {}, \"windows\": {n}, \"accepted\": {hit}, \"far_pct\": {}}}",
            d.name,
            d.refs.len(),
            json_num(pct(hit, n))
        ));
    }
    let p_pooled = pooled_hit as f64 / pooled_n.max(1) as f64;
    md.push_str(&format!(
        "| pooled | - | {pooled_n} | {pooled_hit} | {} |\n\n",
        fmt_opt(pct(pooled_hit, pooled_n), 2)
    ));
    a_rows.push(format!(
        "{{\"set\": \"pooled\", \"windows\": {pooled_n}, \"accepted\": {pooled_hit}, \"far_pct\": {}}}",
        json_num(pct(pooled_hit, pooled_n))
    ));
    json.push_str(&format!(", \"a\": [{}]", a_rows.join(", ")));

    // ---- B ----
    md.push_str(&format!(
        "### B) FAR on long other streams (about {} s each: {STREAM_UNIONS} bystander unions)\n\nTrials = streams x references with stream length >= T. FAR(T) = share with an accepting window ending at or before T. Bound = 1 - (1 - p)^windows(T) from table A's p (consecutive scheme).\n\n",
        STREAM_UNIONS * 4
    ));
    let mut trials: Vec<Vec<Vec<StreamTrial>>> = Vec::new(); // [set][scheme]
    for (si, d) in sets.iter().enumerate() {
        let per_scheme = SCHEMES
            .iter()
            .map(|s| stream_trials(emb, d, *s, check, threshold, &mut memos[si], &mut timings))
            .collect();
        trials.push(per_scheme);
    }
    let mut b_json = Vec::new();
    let pooled_trials: Vec<Vec<StreamTrial>> = (0..SCHEMES.len())
        .map(|s| {
            trials
                .iter()
                .flat_map(|per| per[s].iter().copied())
                .collect()
        })
        .collect();
    let mut tables: Vec<(&str, &[Vec<StreamTrial>], f64)> = Vec::new();
    for (si, d) in sets.iter().enumerate() {
        tables.push((d.name, &trials[si], p_by_set[si]));
    }
    tables.push(("pooled", &pooled_trials, p_pooled));
    for (name, per_scheme, p) in tables {
        md.push_str(&format!(
            "#### {name}\n\n| T s | cons n | cons acc | cons FAR % | bound % | slide n | slide acc | slide FAR % |\n|---|---|---|---|---|---|---|---|\n"
        ));
        let mut rows = Vec::new();
        for t_s in EXPOSURES_S {
            let t = (t_s * SR as f32) as usize;
            let (cn, ch) = far_at(&per_scheme[0], t);
            let (sn, sh) = far_at(&per_scheme[1], t);
            let windows = t / check;
            let bound = 100.0 * (1.0 - (1.0 - p).powi(windows as i32));
            md.push_str(&format!(
                "| {t_s} | {cn} | {ch} | {} | {bound:.2} | {sn} | {sh} | {} |\n",
                fmt_opt(pct(ch, cn), 2),
                fmt_opt(pct(sh, sn), 2)
            ));
            rows.push(format!(
                "{{\"t_s\": {t_s}, \"consecutive\": {{\"n\": {cn}, \"accepted\": {ch}, \"far_pct\": {}}}, \"sliding_500\": {{\"n\": {sn}, \"accepted\": {sh}, \"far_pct\": {}}}, \"independence_bound_pct\": {bound:.4}}}",
                json_num(pct(ch, cn)),
                json_num(pct(sh, sn))
            ));
        }
        md.push('\n');
        b_json.push(format!("\"{name}\": [{}]", rows.join(", ")));
    }
    json.push_str(&format!(", \"b\": {{{}}}", b_json.join(", ")));

    // ---- C ----
    let runs = caller_recall(
        emb,
        test,
        &clips,
        check,
        threshold,
        gains,
        overlaps,
        &mut memos[1],
        &mut timings,
    );
    md.push_str("### C) Caller recall after k s of other speech\n\nTest set, one trial per caller clip: x = other[0..k] ++ caller (other speech x gain mixed into the first overlap ms of the caller; gain `level` = 10^(level_db/20) of the clip's scene; the overlap is truncated where the other stream or the clip ends, `mixed ms (mean)` is what was actually mixed). N = windows scored from the start of x (consecutive with N = 2 is the 0.15.0 behaviour). early_fa = a window ending at or before the onset accepted first; latency = accepting window end - onset.\n\n");
    let mut c_json = Vec::new();
    for scheme in SCHEMES {
        for gain in gains {
            for overlap in overlaps {
                md.push_str(&format!(
                "#### {} / gain {} / overlap {} ms\n\n| gain | mixed ms (mean) | k s | N | n | caller % | early_fa % | miss % | latency p50 ms | latency p90 ms |\n|---|---|---|---|---|---|---|---|---|---|\n",
                scheme.name(),
                gain.label(),
                overlap.label()
            ));
                for run in runs
                    .iter()
                    .filter(|r| r.scheme == scheme && r.gain == *gain && r.overlap == *overlap)
                {
                    let mixed_ms = run.mixed_samples as f64 / run.trials.len().max(1) as f64 / 16.0;
                    for cap in RECALL_CAPS {
                        let c = recall_cell(&run.trials, cap);
                        md.push_str(&format!(
                            "| {} | {mixed_ms:.0} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                            gain.label(),
                            run.k_s,
                            cap_label(cap),
                            c.n,
                            fmt_opt(pct(c.caller, c.n), 1),
                            fmt_opt(pct(c.early, c.n), 1),
                            fmt_opt(pct(c.miss, c.n), 1),
                            fmt_opt(c.lat_p50_ms, 0),
                            fmt_opt(c.lat_p90_ms, 0)
                        ));
                        c_json.push(format!(
                        "{{\"scheme\": \"{}\", \"gain\": {}, \"overlap_ms\": {}, \"mixed_ms_mean\": {mixed_ms:.1}, \"k_s\": {}, \"cap\": {}, \"n\": {}, \"caller_pct\": {}, \"early_fa_pct\": {}, \"miss_pct\": {}, \"latency_p50_ms\": {}, \"latency_p90_ms\": {}}}",
                        scheme.name(),
                        gain.json(),
                        overlap.json(),
                        run.k_s,
                        cap_json(cap),
                        c.n,
                        json_num(pct(c.caller, c.n)),
                        json_num(pct(c.early, c.n)),
                        json_num(pct(c.miss, c.n)),
                        json_num(c.lat_p50_ms),
                        json_num(c.lat_p90_ms)
                    ));
                    }
                }
                md.push('\n');
            }
        }
    }
    json.push_str(&format!(", \"c\": [{}]", c_json.join(", ")));

    // ---- D ----
    timings.sort_by(|a, b| a.total_cmp(b));
    let (p50, p99) = (percentile(&timings, 0.5), percentile(&timings, 0.99));
    md.push_str(&format!(
        "### D) Cost\n\nOne embed of a (check + pre-roll) window over all {} windows computed: p50 {p50:.4} s, p99 {p99:.4} s (one thread).\n\n| scheme | N | windows for a {COST_UTTERANCE_S} s utterance | CPU s at p50 |\n|---|---|---|---|\n",
        timings.len()
    ));
    let mut d_rows = Vec::new();
    for scheme in SCHEMES {
        for cap in COST_CAPS {
            let w = windows_scored(scheme, COST_UTTERANCE_S * SR as usize, check, cap);
            let cpu = w as f64 * p50;
            md.push_str(&format!(
                "| {} | {} | {w} | {cpu:.3} |\n",
                scheme.name(),
                cap_label(cap)
            ));
            d_rows.push(format!(
                "{{\"scheme\": \"{}\", \"cap\": {}, \"windows\": {w}, \"cpu_s_at_p50\": {cpu:.4}}}",
                scheme.name(),
                cap_json(cap)
            ));
        }
    }
    md.push('\n');
    json.push_str(&format!(
        ", \"d\": {{\"embeds\": {}, \"p50_s\": {p50:.6}, \"p99_s\": {p99:.6}, \"utterance_s\": {COST_UTTERANCE_S}, \"rows\": [{}]}}}}",
        timings.len(),
        d_rows.join(", ")
    ));
    Ok((md, json))
}

fn long_other_run(
    fx: &mut Fixtures,
    args: &Args,
    model_files: &[PathBuf],
    manifest_sha: &str,
) -> Result<(), String> {
    let threshold = args
        .fixed_threshold
        .ok_or("--long-other requires --fixed-threshold")?;
    let sha = git_sha();
    let mut md = format!(
        "# Long other-speech bench\n\nthreshold {threshold}, checks {:?} s, pre-roll 300 ms, fixtures `{}`, manifest sha256 `{manifest_sha}`, git {sha}\n\n",
        args.checks,
        args.fixtures.display()
    );
    let mut runs = Vec::new();
    for path in model_files {
        let emb = Embedder::open(path, 1)?;
        for &check_s in &args.checks {
            eprintln!("long-other: model {} check {check_s} s", emb.id);
            let (section, json) = long_other_section(
                fx,
                &emb,
                check_s,
                threshold,
                &args.overlap_gains,
                &args.overlaps,
            )?;
            md.push_str(&section);
            runs.push(json);
        }
    }
    let json = format!(
        "{{\"git_sha\": \"{}\", \"manifest_sha256\": \"{manifest_sha}\", \"fixtures\": \"{}\", \"threshold\": {threshold}, \"runs\": [\n{}\n]}}\n",
        json_escape(&sha),
        json_escape(&args.fixtures.display().to_string()),
        runs.join(",\n")
    );
    let md_path = args.out.join("long_other.md");
    fs::write(&md_path, &md).map_err(|e| format!("write {}: {e}", md_path.display()))?;
    let json_path = args.out.join("long_other.json");
    fs::write(&json_path, json).map_err(|e| format!("write {}: {e}", json_path.display()))?;
    eprint!("{md}");
    eprintln!("--long-other: wrote {}", md_path.display());
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("speaker_bench: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_input_adds_other_only_from_the_onset() {
        let other: Vec<f32> = (1..=10).map(|v| v as f32).collect();
        let clip = vec![100.0; 4];
        let x = build_recall_input(&other, &clip, 3, 2, 1.0);
        // other[0..3] ++ clip, then other[3], other[4] added to the first two clip samples.
        assert_eq!(x, vec![1.0, 2.0, 3.0, 104.0, 105.0, 100.0, 100.0]);
    }

    #[test]
    fn recall_input_applies_the_gain() {
        let other = vec![2.0, 4.0, 6.0, 8.0];
        let clip = vec![10.0, 10.0];
        let x = build_recall_input(&other, &clip, 2, 2, 0.5);
        assert_eq!(x, vec![2.0, 4.0, 13.0, 14.0]);
    }

    #[test]
    fn recall_input_truncates_the_overlap_at_the_end_of_other() {
        let other = vec![1.0, 1.0, 1.0, 1.0];
        let clip = vec![0.0; 5];
        let x = build_recall_input(&other, &clip, 3, 5, 1.0);
        assert_eq!(x, vec![1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(mixed_samples(4, 5, 3, 5), 1);
        assert_eq!(mixed_samples(4, 5, 4, 5), 0);
    }

    #[test]
    fn recall_input_without_overlap_is_a_plain_concat() {
        let other = vec![1.0, 2.0, 3.0, 4.0];
        let clip = vec![9.0, 9.0];
        assert_eq!(
            build_recall_input(&other, &clip, 2, 0, 1.0),
            vec![1.0, 2.0, 9.0, 9.0]
        );
    }

    #[test]
    fn recall_input_overlap_larger_than_the_clip_clamps_to_the_clip() {
        let other = vec![1.0; 10];
        let clip = vec![0.0; 3];
        let x = build_recall_input(&other, &clip, 2, 100, 1.0);
        assert_eq!(x, vec![1.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(mixed_samples(10, 3, 2, 100), 3);
        assert_eq!(OverlapSpec::Full.samples(3), 3);
    }

    #[test]
    fn overlap_and_gain_lists_parse() {
        assert_eq!(
            parse_overlaps("0,1000,full").unwrap(),
            vec![OverlapSpec::Ms(0), OverlapSpec::Ms(1000), OverlapSpec::Full]
        );
        assert_eq!(
            parse_gains("level,0.5").unwrap(),
            vec![GainSpec::Level, GainSpec::Linear(0.5)]
        );
        assert!(parse_gains("-1").is_err());
        assert!(parse_overlaps("x").is_err());
    }

    fn sweep_row(
        margin_db: f32,
        use_speech_level: bool,
        detected: usize,
        merged: usize,
    ) -> SweepRow {
        SweepRow {
            margin_db,
            use_speech_level,
            main_turns: 10,
            detected,
            merged,
            unmatched: 0,
        }
    }

    #[test]
    fn sweep_pick_respects_energy_bound() {
        let rows = [
            sweep_row(4.0, false, 10, 5), // best detection but over the bound
            sweep_row(6.0, false, 8, 2),
            sweep_row(8.0, true, 8, 2),
            sweep_row(8.0, false, 8, 1), // tie on detection: larger margin, then speech level off
        ];
        assert_eq!(pick_sweep(&rows, 2), Some(3));
        assert_eq!(pick_sweep(&rows, 0), None);
        assert_eq!(pick_sweep(&rows, 5), Some(0));
    }

    #[test]
    fn grid_b_labels_use_effective_level() {
        // Mean of -14.9 and -15.4 is -15.15 -> -15.0; the nominal level is only the fallback.
        assert_eq!(effective_label(-14.9 + -15.4, 2, -14), -15.0);
        assert_eq!(effective_label(-20.3, 1, -20), -20.5);
        assert_eq!(effective_label(0.0, 0, -26), -26.0);
    }

    use support::write_wav_stereo_48k;

    #[test]
    fn eer_of_separable_scores_is_zero_and_of_identical_is_half() {
        assert_eq!(eer(&[0.8, 0.9], &[0.1, 0.2]).unwrap().0, 0.0);
        let e = eer(&[0.5, 0.5], &[0.5, 0.5]).unwrap().0;
        assert!((e - 0.5).abs() < 1e-9 || e == 0.5);
    }

    #[test]
    fn threshold_at_far_allows_the_given_share_of_impostors() {
        let impostors: Vec<f32> = (0..200).map(|i| i as f32 / 200.0).collect();
        let t = threshold_at_far(&impostors, 0.01).unwrap();
        let (far, _) = far_frr(&[], &impostors, t);
        assert!(far.unwrap() <= 0.01 + 1e-9);
        // Fewer than 100 impostors: no impostor may be accepted.
        let few = [0.1f32, 0.5, 0.9];
        let t = threshold_at_far(&few, 0.01).unwrap();
        assert_eq!(far_frr(&[], &few, t).0, Some(0.0));
    }

    #[test]
    fn genuine_item_with_enrollment_text_is_skipped() {
        let enroll = vec!["charlie one two three".to_string(), "x".to_string()];
        assert!(shares_enrollment_text("charlie one two three", &enroll));
        assert!(!shares_enrollment_text("bravo four five six", &enroll));
    }

    #[test]
    fn part2_clip_stops_at_utterance_end() {
        assert_eq!(part2_clip_range(0, 800, 1.5), (0, 800 * 16));
        assert_eq!(part2_clip_range(0, 2000, 1.5), (0, 24_000));
    }

    #[test]
    fn manifest_segments_have_expected() {
        let json = r#"{"scenes":[{"id":"s3_test_m10","kind":"conversation","set":"test","level_db":-10,
            "segments":[
              {"start_ms":0,"end_ms":900,"role":"main","voice":"v","sid":0,"text":"a b","enroll":true,
               "kind":"long","expected":{"speaker":"caller","transcript":"a b"}},
              {"start_ms":1000,"end_ms":1900,"role":"babble","voice":"w","sid":3,"text":"x y",
               "kind":"background","expected":{"speaker":"other","transcript":null}}]}]}"#;
        let scenes = load_manifest(json).unwrap();
        let segs = &scenes[0].segments;
        assert_eq!(segs[0].kind, "long");
        assert_eq!(segs[0].expected_speaker, "caller");
        assert_eq!(segs[0].expected_transcript.as_deref(), Some("a b"));
        assert!(segs[1].expected_transcript.is_none());
        // A segment without `expected` is rejected.
        let bad = json.replace(
            "\"expected\":{\"speaker\":\"other\",\"transcript\":null}",
            "\"x\":1",
        );
        assert!(load_manifest(&bad).is_err());
    }

    #[test]
    fn wer_counts_word_edits() {
        assert!((wer("a b c", "a x c") - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(wer("a b c", "a b c"), 0.0);
        assert!((wer("a b c", "") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn merged_utterance_lists_both_segments() {
        // Two turns 0-1000 ms and 1500-2500 ms with one VAD utterance over both.
        let two = [(0, 1000, false), (1500, 2500, false)];
        assert_eq!(merged_segments((0, 2600), &two), vec![0, 1]);
        // A babble segment in between is not a turn.
        let with_babble = [(0, 1000, false), (1100, 1400, true), (1500, 2500, false)];
        assert_eq!(merged_segments((0, 2600), &with_babble), vec![0, 2]);
        // An utterance over one turn only is not merged.
        assert!(merged_segments((0, 1000), &two).is_empty());
    }

    #[test]
    fn pre_roll_matches_product() {
        let config = VadConfig::default();
        let buffer =
            node_webrtc_rust_speech::stt_pre_roll::SttPreRollBuffer::from_vad_config(&config);
        assert_eq!(pre_roll_ms(&config), buffer.capacity_ms());
        assert_eq!(
            pre_roll_ms(&config),
            config.speech_pad_ms + config.min_speech_duration_ms
        );
    }

    #[test]
    fn precomputed_preprocessor_reads_scene_audio() {
        let base = std::env::temp_dir().join(format!("speaker_bench_pre_{}", std::process::id()));
        let (fixtures, pre) = (base.join("fixtures"), base.join("voxlock-live"));
        for d in [&fixtures, &pre] {
            fs::create_dir_all(d.join("scenes")).unwrap();
        }
        // Fixture: 0.1 s of silence; precomputed: 0.2 s of a tone. 48 kHz, mono samples
        // duplicated to stereo by the writer.
        write_wav_stereo_48k(&fixtures.join("scenes/s3_x.wav"), &vec![0.0; 4800]).unwrap();
        write_wav_stereo_48k(&pre.join("scenes/s3_x.wav"), &vec![0.25; 9600]).unwrap();
        let from_fixture =
            read_wav_stereo_48k_pcm(&scene_audio_path(&fixtures, None, "s3_x")).unwrap();
        let from_pre =
            read_wav_stereo_48k_pcm(&scene_audio_path(&fixtures, Some(&pre), "s3_x")).unwrap();
        assert_eq!(from_fixture.len(), 4800 * 4);
        assert_eq!(from_pre.len(), 9600 * 4);
        assert!(from_pre.iter().any(|b| *b != 0));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn precomputed_arg_parses_name_and_dir() {
        assert_eq!(
            parse_precomputed("precomputed:voxlock-live=/tmp/x").unwrap(),
            Some(("voxlock-live".to_string(), PathBuf::from("/tmp/x")))
        );
        assert_eq!(parse_precomputed("gtcrn").unwrap(), None);
        assert!(parse_precomputed("precomputed:nodir").is_err());
        assert!(parse_precomputed("precomputed:=/x").is_err());
    }

    #[test]
    fn cost_caption_labels_precomputed_rows() {
        assert!(!cost_caption(&[]).contains("CPU seconds per audio second"));
        let c = cost_caption(&["voxlock-live".to_string()]);
        assert!(c.contains("voxlock-live") && c.contains("CPU seconds per audio second"));
    }

    #[test]
    fn missing_precomputed_dir_is_skipped() {
        let missing = std::env::temp_dir().join("speaker_bench_no_such_dir_xyz");
        let all = vec![("voxlock-live".to_string(), missing)];
        assert!(usable_precomputed(&all).is_empty());
    }

    #[test]
    fn voxlock_cost_json_becomes_a_cost_row() {
        let json = r#"{"scenes":[{"id":"a","audio_s":10.0,"wall_s":9.0,"cpu_s":8.0},
            {"id":"b","audio_s":10.0,"wall_s":11.0,"cpu_s":12.0}]}"#;
        let row = cost_row_from_json("voxlock-live", json).unwrap();
        assert_eq!(row.name, "voxlock-live");
        assert_eq!(row.frames, 1000);
        assert!((row.rtf_mean - 1.0).abs() < 1e-9);
        assert!(row.streams.is_none() && row.delay_ms.is_none());
        assert!(cost_row_from_json("x", r#"{"scenes":[]}"#).is_err());
    }

    #[test]
    fn onset_delay_median() {
        // Three main turns whose first VAD piece starts 120 ms late, 40 ms late and 50 ms early:
        // the median of the per-turn onsets goes through the real `turn_onset_ms`.
        let utts = [(1120, 3000), (5040, 7000), (8950, 11_000)];
        let turns = [(1000, 3000), (5000, 7000), (9000, 11_000)];
        let delays: Vec<i64> = turns
            .iter()
            .filter_map(|t| turn_onset_ms(t.0, t.1, &utts))
            .collect();
        assert_eq!(delays, vec![120, 40, -50]);
        assert_eq!(median_i64(&delays), Some(40.0));
        // An even count averages the two middle values; no values, no median.
        assert_eq!(median_i64(&[10, 30, 20, 40]), Some(25.0));
        assert_eq!(median_i64(&[]), None);
    }

    #[test]
    fn turn_onset_uses_earliest_overlapping_utterance() {
        assert_eq!(
            turn_onset_ms(1000, 3000, &[(1400, 1800), (2000, 2900)]),
            Some(400)
        );
        assert_eq!(turn_onset_ms(1000, 3000, &[(3100, 3500)]), None);
        assert_eq!(turn_onset_ms(1000, 3000, &[(900, 1500)]), Some(-100));
    }

    #[test]
    fn default_level_scene_keeps_parts_1_and_2_at_minus_20() {
        assert!(default_level_scene("conversation", -20));
        assert!(!default_level_scene("conversation", -26));
        assert!(!default_level_scene("conversation", -14));
        assert!(default_level_scene("main_over_babble", -20));
        assert!(default_level_scene("bystander_alone", -20));
    }

    #[test]
    fn delay_scene_prefers_clean_s1() {
        assert_eq!(
            delay_scene(&["conversation", "main_over_babble", "main_clean"]),
            Some(2)
        );
        assert_eq!(delay_scene(&["conversation", "main_over_babble"]), Some(1));
        assert_eq!(delay_scene(&["conversation"]), None);
    }

    #[test]
    fn cosine_of_parallel_vectors_is_one() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn window_ends_per_scheme_and_cap() {
        let check = 24_000; // 1.5 s
        let len = 5 * 16_000; // 5 s
        assert_eq!(
            window_ends(Scheme::Consecutive, len, check),
            vec![24_000, 48_000, 72_000]
        );
        assert_eq!(
            window_ends(Scheme::Sliding500, len, check),
            vec![24_000, 32_000, 40_000, 48_000, 56_000, 64_000, 72_000, 80_000]
        );
        assert!(window_ends(Scheme::Consecutive, 23_999, check).is_empty());
        let ends = window_ends(Scheme::Sliding500, len, check);
        assert_eq!(capped(&ends, Some(2)), &[24_000, 32_000]);
        assert_eq!(capped(&ends, Some(99)).len(), ends.len());
        assert_eq!(capped(&ends, None).len(), ends.len());
        assert_eq!(
            windows_scored(Scheme::Consecutive, 17 * 16_000, check, None),
            11
        );
        assert_eq!(
            windows_scored(Scheme::Sliding500, 17 * 16_000, check, Some(4)),
            4
        );
    }

    #[test]
    fn window_range_clamps_at_zero_with_pre_roll() {
        let check = 24_000;
        assert_eq!(window_range(24_000, check), (0, 24_000));
        assert_eq!(window_range(28_800, check), (0, 28_800));
        assert_eq!(window_range(48_000, check), (48_000 - 28_800, 48_000));
    }

    #[test]
    fn speech_union_merges_overlaps_and_cuts_gaps() {
        assert_eq!(
            merge_spans(&[(500, 700), (0, 100), (50, 150), (150, 200), (300, 300)]),
            vec![(0, 200), (500, 700)]
        );
        let audio: Vec<f32> = (0..1000 * 16).map(|i| i as f32).collect();
        let union = speech_union(&audio, &[(10, 12), (11, 13), (20, 21)]);
        // [10, 13) ms then [20, 21) ms, gap cut.
        assert_eq!(union.len(), (3 + 1) * 16);
        assert_eq!(union[0], 160.0);
        assert_eq!(union[3 * 16], 320.0);
    }

    #[test]
    fn stitch_wraps_modulo_the_number_of_unions() {
        let unions = vec![vec![0.0], vec![1.0], vec![2.0]];
        assert_eq!(stitch(&unions, 0, 4), vec![0.0, 1.0, 2.0, 0.0]);
        assert_eq!(stitch(&unions, 2, 4), vec![2.0, 0.0, 1.0, 2.0]);
    }

    #[test]
    fn far_at_counts_only_long_enough_trials_with_an_accept_by_t() {
        let trials = [
            StreamTrial {
                len: 100,
                first_accept: Some(40),
            },
            StreamTrial {
                len: 100,
                first_accept: Some(90),
            },
            StreamTrial {
                len: 100,
                first_accept: None,
            },
            StreamTrial {
                len: 50,
                first_accept: Some(10),
            },
        ];
        assert_eq!(far_at(&trials, 30), (4, 1));
        assert_eq!(far_at(&trials, 40), (4, 2));
        assert_eq!(far_at(&trials, 60), (3, 1));
        assert_eq!(far_at(&trials, 100), (3, 2));
        assert_eq!(far_at(&trials, 101), (0, 0));
    }

    #[test]
    fn first_accept_end_takes_the_earliest_window() {
        assert_eq!(
            first_accept_end(&[10, 20, 30], &[false, true, true]),
            Some(20)
        );
        assert_eq!(first_accept_end(&[10, 20], &[false, false]), None);
    }

    #[test]
    fn classify_early_fa_caller_and_miss() {
        let ends = [100, 200, 300, 400];
        let onset = 200;
        // Accept exactly at the onset is an early false accept.
        assert_eq!(
            classify(&ends, &[false, true, false, false], onset, None),
            Outcome::EarlyFa
        );
        assert_eq!(
            classify(&ends, &[true, false, false, false], onset, None),
            Outcome::EarlyFa
        );
        // First accept after the onset: caller, latency = end - onset.
        assert_eq!(
            classify(&ends, &[false, false, true, true], onset, None),
            Outcome::Caller(100)
        );
        assert_eq!(
            classify(&ends, &[false, false, false, false], onset, None),
            Outcome::Miss
        );
        // The cap hides the later accept.
        assert_eq!(
            classify(&ends, &[false, false, false, true], onset, Some(3)),
            Outcome::Miss
        );
        assert_eq!(
            classify(&ends, &[false, false, false, true], onset, Some(4)),
            Outcome::Caller(200)
        );
    }

    #[test]
    fn recall_cell_counts_outcomes_and_latency_percentiles() {
        let trial = |accepts: [bool; 3]| TrialScores {
            onset: 16_000,
            ends: vec![16_000, 32_000, 48_000],
            accepts: accepts.to_vec(),
        };
        let trials = [
            trial([false, true, true]),   // caller, 1000 ms
            trial([false, false, true]),  // caller, 2000 ms
            trial([true, false, false]),  // early_fa
            trial([false, false, false]), // miss
        ];
        let c = recall_cell(&trials, None);
        assert_eq!((c.n, c.caller, c.early, c.miss), (4, 2, 1, 1));
        assert_eq!(c.lat_p50_ms, Some(2000.0)); // round(0.5) of two values picks the upper one
        assert_eq!(c.lat_p90_ms, Some(2000.0));
        let capped = recall_cell(&trials, Some(2));
        assert_eq!((capped.caller, capped.early, capped.miss), (1, 1, 2));
        assert_eq!(capped.lat_p50_ms, Some(1000.0));
    }
}
