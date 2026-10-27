//! Speaker-ID bench on the scenes of `speaker_fixtures`: how well does a speaker-embedding model
//! tell the main voice from quieter background voices, and what does that do to what the voice
//! agent would transcribe and attribute?
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_bench -- \
//!   --fixtures <speaker_fixtures out dir> --models <speaker dir> --stt-model <zipformer dir> \
//!   --out <dir> [--vad-min-silence-ms <n>]
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
//! 48 kHz stereo PCM through the speech crate's `VadEngine` with `VadConfig::default()` (energy VAD;
//! only `min_silence_duration_ms` may be overridden with `--vad-min-silence-ms`). For each VAD
//! utterance:
//! - the Zipformer online recognizer transcribes the VAD window (pre-roll + speech + trailing
//!   silence) and the transcript is scored with the roundtrip oracle (`wordSimilarity`);
//! - the first `check` seconds from the start of the speech are embedded (model and check length
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
//! `NWR_BENCH_GIT_SHA` or `git rev-parse HEAD`, manifest sha256) and `<out>/summary.md`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use node_webrtc_rust_speech::config::VadConfig;
use node_webrtc_rust_speech::vad::{VadEngine, VadTransition};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

#[path = "support/mod.rs"]
mod support;

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

struct Seg {
    start_ms: usize,
    end_ms: usize,
    role: String,
    text: String,
    enroll: bool,
}

struct SceneInfo {
    id: String,
    kind: String,
    set: Set,
    level_db: i32,
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
            segments.push(Seg {
                start_ms: num_of(seg, "start_ms")? as usize,
                end_ms: num_of(seg, "end_ms")? as usize,
                role: str_of(seg, "role")?,
                text: str_of(seg, "text")?,
                enroll: matches!(seg.get("enroll"), Some(Json::Bool(true))),
            });
        }
        out.push(SceneInfo {
            id: str_of(scene, "id")?,
            kind: str_of(scene, "kind")?,
            set,
            level_db: num_of(scene, "level_db")? as i32,
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
        let s3 = |i: &usize| self.scenes[*i].kind == "conversation";
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

fn collect_items(fx: &mut Fixtures) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    for scene in 0..fx.scenes.len() {
        let kind = fx.scenes[scene].kind.clone();
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

/// Enrollment of a scene: its enrollment segments concatenated.
fn enrollment(fx: &mut Fixtures, scene: usize) -> Result<Vec<f32>, String> {
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
    let audio = fx.audio16(scene)?.clone();
    let mut out = Vec::new();
    for (start, end) in ranges {
        out.extend_from_slice(&audio[(start * 16).min(audio.len())..(end * 16).min(audio.len())]);
    }
    Ok(out)
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
    main_recognized: usize,
    main_caller: usize,
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
}

struct Utt {
    /// Start of speech (SpeechStart minus the minimum speech duration).
    start_ms: usize,
    /// End of the last active frame.
    end_active_ms: usize,
    /// SpeechEnd frame (includes the trailing silence).
    window_end_ms: usize,
}

fn run_vad(pcm: &[u8], config: &VadConfig) -> Result<Vec<Utt>, String> {
    let mut vad = VadEngine::new(config.clone()).map_err(|e| e.to_string())?;
    let frame_bytes = 960 * 2 * 2; // 20 ms of 48 kHz stereo s16le
    let mut utterances = Vec::new();
    let mut start: Option<usize> = None;
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
                    start = Some((t + 20).saturating_sub(config.min_speech_duration_ms as usize));
                }
                VadTransition::SpeechEnd => {
                    if let Some(s) = start.take() {
                        utterances.push(Utt {
                            start_ms: s,
                            end_active_ms: last_active_end.max(s),
                            window_end_ms: t + 20,
                        });
                    }
                }
            }
        }
    }
    if let Some(s) = start {
        utterances.push(Utt {
            start_ms: s,
            end_active_ms: last_active_end.max(s),
            window_end_ms: total_ms,
        });
    }
    Ok(utterances)
}

fn overlap_ms(a: (usize, usize), b: (usize, usize)) -> usize {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

struct Verdict {
    seg: Option<usize>,
    overlap: usize,
    transcript: String,
    caller: bool,
}

fn words_of(text: &str) -> Vec<String> {
    normalize_for_compare(text)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn part2_scene(
    fx: &mut Fixtures,
    scene: usize,
    emb: &Embedder,
    check_s: f32,
    threshold: f32,
    recognizer: &sherpa_onnx::OnlineRecognizer,
    vad_config: &VadConfig,
    leak_vocab: &HashSet<String>,
) -> Result<LevelStats, String> {
    let id = fx.scenes[scene].id.clone();
    let pcm = read_wav_stereo_48k_pcm(&fx.dir.join("scenes").join(format!("{id}.wav")))?;
    let audio = fx.audio16(scene)?.clone();
    let enroll = enrollment(fx, scene)?;
    let (reference, _) = emb.embed(&enroll).ok_or("enrollment too short to embed")?;
    let utterances = run_vad(&pcm, vad_config)?;

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
    let mut stats = LevelStats {
        utterances: utterances.len(),
        ..Default::default()
    };
    let mut verdicts: Vec<Verdict> = Vec::new();
    for utt in &utterances {
        let window_from = utt
            .start_ms
            .saturating_sub(vad_config.speech_pad_ms as usize)
            * 16;
        let window_to = (utt.window_end_ms * 16).min(audio.len());
        let mut window = audio[window_from.min(window_to)..window_to].to_vec();
        window.extend(std::iter::repeat_n(0.0f32, TAIL_SILENCE_MS * 16));
        let transcript = recognize(recognizer, &window);

        let (from, to) = part2_clip_range(utt.start_ms, utt.window_end_ms, check_s);
        let from = from.min(audio.len());
        let clip = &audio[from..to.min(audio.len()).max(from)];
        let caller = emb
            .embed(clip)
            .is_some_and(|(e, _)| cosine(&reference, &e) > threshold);

        let span = (utt.start_ms, utt.end_active_ms);
        let len = span.1.saturating_sub(span.0).max(1);
        let mut best: Option<(usize, usize)> = None;
        for (i, (s, e, role, _, _)) in segs.iter().enumerate() {
            if role == "babble" {
                continue;
            }
            let o = overlap_ms(span, (*s, *e));
            if o > 0 && best.is_none_or(|(_, bo)| o > bo) {
                best = Some((i, o));
            }
        }
        let matched = best.filter(|(_, o)| o * 2 >= len);
        if matched.is_none() {
            stats.unmatched += 1;
            if caller {
                stats.unmatched_caller += 1;
            } else {
                stats.unmatched_other += 1;
            }
        }
        verdicts.push(Verdict {
            seg: matched.map(|m| m.0),
            overlap: matched.map_or(0, |m| m.1),
            transcript,
            caller,
        });
    }

    for (i, (_, _, role, enroll, text)) in segs.iter().enumerate() {
        if *enroll || role == "babble" {
            continue;
        }
        let mine: Vec<&Verdict> = verdicts.iter().filter(|v| v.seg == Some(i)).collect();
        // Utterances are in time order; the attribution of a turn is its best-overlapping one.
        let best = mine.iter().max_by_key(|v| v.overlap).copied();
        if role == "main" {
            stats.main_turns += 1;
            let Some(best) = best else {
                stats.main_missed += 1;
                continue;
            };
            let transcript = mine
                .iter()
                .map(|v| v.transcript.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if word_similarity(text, &transcript) >= ORACLE_MIN {
                stats.main_recognized += 1;
            }
            if best.caller {
                stats.main_caller += 1;
            }
            let words = words_of(&transcript);
            stats.main_words += words.len();
            stats.leak_words += words.iter().filter(|w| leak_vocab.contains(*w)).count();
        } else {
            stats.byst_turns += 1;
            match best {
                None => stats.byst_missed += 1,
                Some(v) if v.caller => stats.byst_caller += 1,
                Some(_) => stats.byst_other += 1,
            }
        }
    }
    Ok(stats)
}

fn add_stats(into: &mut LevelStats, other: &LevelStats) {
    into.main_turns += other.main_turns;
    into.main_missed += other.main_missed;
    into.main_recognized += other.main_recognized;
    into.main_caller += other.main_caller;
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

struct Args {
    fixtures: PathBuf,
    models: PathBuf,
    stt_model: PathBuf,
    out: PathBuf,
    vad_min_silence_ms: Option<u32>,
}

fn parse_args() -> Result<Args, String> {
    let (mut fixtures, mut models, mut stt, mut out, mut silence) = (None, None, None, None, None);
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
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        fixtures: fixtures.ok_or("--fixtures is required")?,
        models: models.ok_or("--models is required")?,
        stt_model: stt.ok_or("--stt-model is required")?,
        out: out.ok_or("--out is required")?,
        vad_min_silence_ms: silence,
    })
}

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
        .filter(|i| fx.scenes[*i].kind == "conversation" && fx.scenes[*i].set == Set::Test)
        .collect();
    let (part2_scenes, part2_set) = if s3_test.is_empty() {
        (
            (0..fx.scenes.len())
                .filter(|i| fx.scenes[*i].kind == "conversation")
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

    let mut vad_config = VadConfig::default();
    if let Some(ms) = args.vad_min_silence_ms {
        vad_config.min_silence_duration_ms = ms;
    }
    let recognizer = create_recognizer(&args.stt_model)?;
    let mut by_level: BTreeMap<i32, LevelStats> = BTreeMap::new();
    for scene in part2_scenes {
        let level = fx.scenes[scene].level_db;
        eprintln!("part 2 scene {}", fx.scenes[scene].id);
        let stats = part2_scene(
            &mut fx,
            scene,
            &embedders[best_model],
            best_check,
            best_threshold,
            &recognizer,
            &vad_config,
            &leak_vocab,
        )?;
        add_stats(by_level.entry(level).or_default(), &stats);
    }

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
        "  \"part2\": {{\n    \"model\": \"{}\",\n    \"check_s\": {},\n    \"threshold\": {:.6},\n    \"set\": \"{}\",\n    \"vad_min_silence_ms\": {},\n    \"levels\": [\n",
        json_escape(&embedders[best_model].id), best_check, best_threshold, json_escape(part2_set),
        vad_config.min_silence_duration_ms
    ));
    let level_count = by_level.len();
    for (i, (level, s)) in by_level.iter().enumerate() {
        json.push_str(&format!(
            "      {{\"level_db\": {level}, \"vad_utterances\": {}, \"main_turns\": {}, \"main_missed\": {}, \"a_main_recognized_pct\": {}, \"b_main_caller_pct\": {}, \"bystander_turns\": {}, \"bystander_missed\": {}, \"c_bystander_other_pct\": {}, \"d_bystander_caller_pct\": {}, \"e_leakage_pct\": {}, \"f_unmatched_utterances\": {}, \"f_unmatched_caller\": {}, \"f_unmatched_other\": {}}}{}\n",
            s.utterances, s.main_turns, s.main_missed,
            json_num(pct(s.main_recognized, s.main_turns)),
            json_num(pct(s.main_caller, s.main_turns)),
            s.byst_turns, s.byst_missed,
            json_num(pct(s.byst_other, s.byst_turns)),
            json_num(pct(s.byst_caller, s.byst_turns)),
            json_num(pct(s.leak_words, s.main_words)),
            s.unmatched, s.unmatched_caller, s.unmatched_other,
            if i + 1 < level_count { "," } else { "" }
        ));
    }
    json.push_str("    ]\n  }\n}\n");
    fs::write(args.out.join("results.json"), json)
        .map_err(|e| format!("write results.json: {e}"))?;

    let mut md = String::new();
    md.push_str("# Speaker-ID bench\n\n");
    md.push_str(&format!(
        "nproc {nproc}, CPU {cpu}, git {sha}, manifest sha256 `{manifest_sha}`\n\n"
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
        "\n## Part 2: end to end\n\nModel `{}`, check {} s, threshold {:.4} (tune set, FAR 1 %). Scenes: {}. VAD `VadConfig::default()` with min_silence {} ms.\n\n",
        embedders[best_model].id, best_check, best_threshold, part2_set, vad_config.min_silence_duration_ms
    ));
    md.push_str("| level | VAD utt | main turns (missed) | (a) recognized % | (b) caller % | bystander turns (missed) | (c) other % | (d) caller % | (e) leakage % | (f) unmatched utt (caller/other) |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for (level, s) in &by_level {
        md.push_str(&format!(
            "| {level} dB | {} | {} ({}) | {} | {} | {} ({}) | {} | {} | {} | {} ({}/{}) |\n",
            s.utterances,
            s.main_turns,
            s.main_missed,
            fmt_opt(pct(s.main_recognized, s.main_turns), 1),
            fmt_opt(pct(s.main_caller, s.main_turns), 1),
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
    fn cosine_of_parallel_vectors_is_one() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }
}
