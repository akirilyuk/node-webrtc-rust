//! Speaker-ID bench on the scenes of `speaker_fixtures`: how well does a speaker-embedding model
//! tell the main voice from quieter background voices, and what does that do to what the voice
//! agent would transcribe and attribute?
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_bench -- \
//!   --fixtures <speaker_fixtures out dir> --models <speaker dir> --stt-model <zipformer dir> \
//!   --out <dir> [--vad-min-silence-ms <n>] \
//!   [--enhance <denoiser models dir> [--pre raw,rnnoise,gtcrn,...] [--cost-seconds <n>] [--streams-seconds <n>]]
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
    fn open(path: &Path) -> Result<Self, String> {
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
            timings: vec![Vec::new(); CHECKS_S.len()],
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
    /// Up to 1.5 s from the start of the segment.
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
            let to = (end_ms * 16).min(audio.len()).min(from + MAX_CHECK_SAMPLES);
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

fn part2_scene(fx: &mut Fixtures, scene: usize, p: &P2) -> Result<(LevelStats, Vec<Row>), String> {
    let id = fx.scenes[scene].id.clone();
    let level_db = fx.scenes[scene].level_db;
    let raw_pcm = read_wav_stereo_48k_pcm(&fx.dir.join("scenes").join(format!("{id}.wav")))?;
    // Part 2: the scene as generated (16 kHz wav for STT and embedding). Part 3: the product
    // order, preprocessor and AGC on the 48 kHz stereo frames first, then VAD on that PCM and
    // STT / embedding on its mono 16 kHz conversion; the enrollment is cut from the same signal.
    let (pcm, audio, underruns, pre_name, agc_on) = match p.enhance {
        None => (raw_pcm, fx.audio16(scene)?.clone(), 0, "part2", false),
        Some(e) => {
            let mut pre = make_preprocessor(e.pre_name, e.models)?;
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
    emb: &'a Embedder,
    check_s: f32,
    threshold: f32,
    recognizer: &'a sherpa_onnx::OnlineRecognizer,
    vad_config: &'a VadConfig,
    leak_vocab: &'a HashSet<String>,
    cost_seconds: usize,
    streams_seconds: usize,
}

struct GridRow {
    pre: String,
    agc: bool,
    main_db: i32,
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
    model: String,
    check_s: f32,
    threshold: f32,
    set: &'static str,
    pre_names: Vec<String>,
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

fn part3_run(fx: &mut Fixtures, r: &Part3Run, rows: &mut Vec<Row>) -> Result<Part3, String> {
    let pre_names: Vec<&str> = ALL_PRE
        .iter()
        .copied()
        .filter(|p| r.pre_filter.iter().any(|f| f == p))
        .collect();
    if pre_names.is_empty() {
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

    let mut grid = Vec::new();
    for name in &pre_names {
        for agc in [false, true] {
            let in_b = GRID_B_PRE.contains(name);
            let mut by: BTreeMap<(i32, i32), LevelStats> = BTreeMap::new();
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
                };
                let p2 = P2 {
                    enhance: Some(&enhance),
                    emb: r.emb,
                    check_s: r.check_s,
                    threshold: r.threshold,
                    recognizer: r.recognizer,
                    vad_name: "energy",
                    vad_config: r.vad_config,
                    leak_vocab: r.leak_vocab,
                };
                eprintln!(
                    "part 3 pre {name} agc {} scene {}",
                    if agc { "on" } else { "off" },
                    fx.scenes[scene].id
                );
                let (stats, scene_rows) = part2_scene(fx, scene, &p2)?;
                add_stats(by.entry((main_db, level)).or_default(), &stats);
                rows.extend(scene_rows);
            }
            eprintln!(
                "part 3 pre {name} agc {} done in {:.0} s",
                on_off(agc),
                combo_started.elapsed().as_secs_f64()
            );
            for ((main_db, level_db), stats) in by {
                grid.push(GridRow {
                    pre: (*name).to_string(),
                    agc,
                    main_db,
                    level_db,
                    stats,
                });
            }
        }
    }
    let wer = s1_wer(fx, r, &pre_names)?;
    let cost = cost_rows(fx, r, &pre_names, &scenes)?;
    Ok(Part3 {
        model: r.emb.id.clone(),
        check_s: r.check_s,
        threshold: r.threshold,
        set: set_label,
        pre_names: pre_names.iter().map(|p| (*p).to_string()).collect(),
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

fn part3_md(p: &Part3) -> String {
    let mut md = String::new();
    md.push_str(&format!(
        "\n## Part 3: denoisers and caller AGC\n\nSpeaker model `{}`, check {} s, threshold {:.4} (tune set, FAR 1 %). Scenes: {}. Pipeline per combo: preprocessor, then AGC if on, on the 20 ms 48 kHz stereo frames; energy VAD (`VadConfig::default()`) on that PCM; STT and embedding on its mono 16 kHz conversion, starting at SpeechStart minus the product pre-roll. `turns.jsonl` has every turn (`pre`, `agc`); `conversations.md` shows raw/off, gtcrn/on and dpdfnet2/on.\n",
        p.model, p.check_s, p.threshold, p.set
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
        .filter(|n| GRID_B_PRE.contains(&n.as_str()))
    {
        for agc in [false, true] {
            for g in p.grid.iter().filter(|g| {
                g.pre == *name && g.agc == agc && (g.level_db == -15 || g.level_db == -10)
            }) {
                md.push_str(&format!(
                    "| {name} | {} | {} | {} dB | {} |\n",
                    on_off(agc),
                    g.main_db,
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
    md.push_str("\n### Cost (one core, 20 ms frames)\n\nRTF = processing time / 20 ms per frame. Streams per core = the largest N (steps 1, 2, 4, 8, 12, 16, 24, 32) of real-time paced streams on one core with at most 1 % of frames finishing more than 20 ms after they were due; `+` marks the last step passed. Delay = lag of the cross-correlation of input and output (0 to 800 ms). `agc` is the AGC alone.\n\n");
    md.push_str("| pre | frames | RTF p50 | RTF p99 | RTF mean | streams per core | delay ms |\n|---|---|---|---|---|---|---|\n");
    for c in &p.cost {
        md.push_str(&format!(
            "| {} | {} | {:.4} | {:.4} | {:.4} | {} | {} |\n",
            c.name,
            c.frames,
            c.rtf_p50,
            c.rtf_p99,
            c.rtf_mean,
            c.streams.map_or("-".to_string(), |n| format!(
                "{n}{}",
                if c.streams_capped { "+" } else { "" }
            )),
            fmt_opt(c.delay_ms, 1),
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

fn part3_json(p: &Part3) -> String {
    let mut j = String::new();
    j.push_str(&format!(
        "  \"part3\": {{\n    \"model\": \"{}\",\n    \"check_s\": {},\n    \"threshold\": {:.6},\n    \"set\": \"{}\",\n    \"grid\": [\n",
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
                "      {{\"pre\": \"{}\", \"agc\": {}, \"main_db\": {}, \"level_db\": {}, {}}}",
                g.pre,
                g.agc,
                g.main_db,
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
    name: &'static str,
    min_silence_ms: u32,
    pre_roll_ms: u32,
    levels: BTreeMap<i32, LevelStats>,
}

struct Args {
    fixtures: PathBuf,
    models: PathBuf,
    stt_model: PathBuf,
    out: PathBuf,
    vad_min_silence_ms: Option<u32>,
    /// Part 3: directory with the denoiser models (`.models/enhance`).
    enhance: Option<PathBuf>,
    /// Part 3: preprocessors to run (default: all of [`ALL_PRE`]).
    pre: Vec<String>,
    /// Part 3 cost: seconds of S3 audio for the single-stream RTF.
    cost_seconds: usize,
    /// Part 3 cost: seconds each streams-per-core step runs.
    streams_seconds: usize,
}

fn parse_args() -> Result<Args, String> {
    let (mut fixtures, mut models, mut stt, mut out, mut silence) = (None, None, None, None, None);
    let mut enhance = None;
    let mut pre: Vec<String> = ALL_PRE.iter().map(|p| (*p).to_string()).collect();
    let (mut cost_seconds, mut streams_seconds) = (60usize, 10usize);
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
            "--enhance" => enhance = Some(PathBuf::from(value("--enhance")?)),
            "--pre" => {
                pre = value("--pre")?
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
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
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        fixtures: fixtures.ok_or("--fixtures is required")?,
        models: models.ok_or("--models is required")?,
        stt_model: stt.ok_or("--stt-model is required")?,
        out: out.ok_or("--out is required")?,
        vad_min_silence_ms: silence,
        enhance,
        pre,
        cost_seconds,
        streams_seconds,
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
    if model_files.is_empty() {
        return Err(format!(
            "no .onnx embedding model in {}",
            args.models.display()
        ));
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
    for (m, path) in model_files.iter().enumerate() {
        let mut emb = Embedder::open(path)?;
        eprintln!("model {}", emb.id);
        let mut refs_needed: Vec<usize> = items.iter().map(|i| i.ref_scene).collect();
        refs_needed.sort_unstable();
        refs_needed.dedup();
        for scene in refs_needed {
            let enroll = enrollment(&mut fx, scene)?;
            let (e, _) = emb.embed(&enroll).ok_or("enrollment too short to embed")?;
            ref_cache.insert((m, scene), e);
        }
        for (c, check_s) in CHECKS_S.iter().enumerate() {
            let mut scores = Vec::with_capacity(items.len());
            for item in &items {
                let scored = emb.embed(first(&item.clip, check_samples(*check_s)));
                scores.push(scored.map(|(e, secs)| {
                    emb.timings[c].push(secs);
                    cosine(&ref_cache[&(m, item.ref_scene)], &e)
                }));
            }
            p1_rows.extend(part1_rows(&emb.id, *check_s, &items, &scores));
            if let Some(row) = real_voice_row(&emb, &args.models, *check_s)? {
                real_rows.push(row);
            }
        }
        embedders.push(emb);
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
    let mut outcomes: Vec<VariantOutcome> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    {
        let name = "energy";
        let p2 = P2 {
            enhance: None,
            emb: &embedders[best_model],
            check_s: best_check,
            threshold: best_threshold,
            recognizer: &recognizer,
            vad_name: name,
            vad_config: &vad_config,
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
            name,
            min_silence_ms: vad_config.min_silence_duration_ms,
            pre_roll_ms: pre_roll_ms(&vad_config),
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
            let run3 = Part3Run {
                models,
                pre_filter: &args.pre,
                emb: &embedders[model_idx],
                check_s,
                threshold,
                recognizer: &recognizer,
                vad_config: &vad_config,
                leak_vocab: &leak_vocab,
                cost_seconds: args.cost_seconds,
                streams_seconds: args.streams_seconds,
            };
            Some(part3_run(&mut fx, &run3, &mut rows)?)
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
        for (c, check_s) in CHECKS_S.iter().enumerate() {
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
                        "          {{\"level_db\": {level}, \"vad_utterances\": {}, \"merged_utterances\": {}, \"main_turns\": {}, \"main_missed\": {}, \"qualified_turns\": {}, \"a_qualified_oracle_pct\": {}, \"long_turns\": {}, \"long_wer_pct\": {}, \"long_word_similarity_pct\": {}, \"b_main_caller_pct\": {}, \"b_qualified_caller_pct\": {}, \"b_long_caller_pct\": {}, \"bystander_turns\": {}, \"bystander_missed\": {}, \"c_bystander_other_pct\": {}, \"d_bystander_caller_pct\": {}, \"e_leakage_pct\": {}, \"f_unmatched_utterances\": {}, \"f_unmatched_caller\": {}, \"f_unmatched_other\": {}}}{}\n",
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
                        if i + 1 < level_count { "," } else { "" }
                    ));
                }
                json.push_str(&format!("        ]\n      }}{comma}\n"));
            }
        }
    }
    json.push_str("    }\n  }");
    if let Some(p3) = &part3 {
        json.push_str(",\n");
        json.push_str(&part3_json(p3));
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
        for check_s in CHECKS_S {
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
                md.push_str("| level | VAD utt | merged utt | main turns (missed) | (a) oracle % (qualified) | WER % (long, measured, not qualified) | (b) main to caller %: all / qualified / long | bystander turns (missed) | (c) other % | (d) caller % | (e) leakage % | (f) unmatched utt (caller/other) |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
                for (level, s) in by_level {
                    md.push_str(&format!(
                        "| {level} dB | {} | {} | {} ({}) | {} | {} | {} / {} / {} | {} ({}) | {} | {} | {} | {} ({}/{}) |\n",
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
                        s.unmatched_other
                    ));
                }
            }
        }
    }
    if let Some(p3) = &part3 {
        md.push_str(&part3_md(p3));
    }
    fs::write(args.out.join("summary.md"), md).map_err(|e| format!("write summary.md: {e}"))?;
    eprintln!("wrote {}/results.json and summary.md", args.out.display());
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
}
