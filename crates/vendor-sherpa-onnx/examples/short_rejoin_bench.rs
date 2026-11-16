//! Short-rejoin bench for the speaker gate (plan `2026-11-13-speaker-split-other-to-caller`,
//! chunk 7c, decision dd): can the audio of a key's `short` checks (less VAD speech than
//! `min_check_speech_ms`, never scored today) tell a bystander from the caller well enough to split
//! at the VAD gap when the caller rejoins that key?
//!
//! eu-load 43698709106 iteration 20: VAD caught only the tail of a bystander turn, in two `short`
//! segments. The caller's onset rejoined that key (`reopen_short`), so the only scored windows were
//! caller audio and the bystander's words landed in a caller final. The candidate rule scores the
//! short audio (the voiced parts of the key's short segments, concatenated) before the rejoin and
//! splits only when the score is below a "confident other" bound. A paused caller ("stop ... wait")
//! goes through the same path with its own short audio, so the bar is that a caller's short audio
//! never scores below the bound.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example short_rejoin_bench -- \
//!   --voices-root <.models> --speaker-dir <.models/speaker> --bed <tocumen48.f32> \
//!   --cache <dir> --librispeech <LibriSpeech dir> [--bed-db -12] [--out <file.md>]
//! ```
//!
//! - Corpora (decision u): Piper (`en_US-amy-low` sid 0 plus `TEST_VOICES` and `TUNE_VOICES`,
//!   11 voices) and LibriSpeech `test-clean` (40 speakers, decoded wav next to the flac files).
//! - Per speaker: an enrollment clip of 3000 ms from the voiced onset (the gate's
//!   `enroll_speech_ms`, no pre-roll) and a talk stream. Piper: enrollment from two long
//!   sentences, talk from the long background sentences and long sentences 8-16 (as
//!   `false_accept_bench`). LibriSpeech: same-session, the chapter with the most audio, its first
//!   3 s from the voiced onset enroll, the rest (up to 60 s) is the talk. Voices at -20 dBFS.
//! - Conditions: `bed` (Tocumen airport bed at `--bed-db` re the voice, under enrollment and talk)
//!   and `clean`.
//! - Short audio of voiced length `L` (20 ms frames above -45 dBFS on the clean voice):
//!   `1 seg` = 300 ms pre-roll plus the span from a voiced frame until `L` voiced ms;
//!   `2 seg` = two such spans of `L/2` from distant points of the talk, concatenated (the second
//!   without pre-roll), as two short segments of one key. [`POSITIONS`] start points per speaker.
//! - Scores: TitaNet-small raw cosine against the enrollment of the same condition.
//!   **Paused caller** = the speaker's own short audio against its own enrollment: a score below
//!   the bound would split a caller (must be 0). **Bystander** = every other speaker's short audio
//!   against the enrollment: a score below the bound splits (higher is better). Bystander audio
//!   at or above the gate threshold 0.459 is reported as "scores caller".

use std::collections::HashMap;
use std::fmt::Write as _;
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
const FRAME_MS: usize = 20;
const PRE_ROLL_MS: usize = 300;
const ENROLL_MS: usize = 3000;
const GATE_THRESHOLD: f32 = 0.459;
const MAIN_DB: f32 = -20.0;
const VOICED_DB: f32 = -45.0;
const GAP_MS: usize = 200;
const LIBRI_STREAM_S: usize = 60;
/// Voiced lengths of the short audio (ms).
const LENGTHS: [usize; 6] = [400, 600, 800, 1000, 1200, 1500];
/// "Confident other" bounds swept (score below the bound splits).
const BOUNDS: [f32; 6] = [0.05, 0.10, 0.15, 0.20, 0.25, 0.30];
const POSITIONS: usize = 12;
const SHAPES: [usize; 2] = [1, 2];

const LONG: &str = include_str!("support/long_sentences.txt");
const LONG_BACKGROUND: &str = include_str!("support/long_background_sentences.txt");
const AMY: Voice = voice("en_US-amy-low", "vits-piper-en_US-amy-low", 0);

struct Embedder {
    extractor: SpeakerEmbeddingExtractor,
    times: Vec<f64>,
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
        Ok(Self {
            extractor,
            times: Vec::new(),
        })
    }

    fn embed(&mut self, samples: &[f32]) -> Option<Vec<f32>> {
        let start = Instant::now();
        let stream = self.extractor.create_stream()?;
        stream.accept_waveform(SR, samples);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return None;
        }
        let e = self.extractor.compute(&stream)?;
        self.times.push(start.elapsed().as_secs_f64() * 1000.0);
        Some(e)
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

/// Piper clips, cached on disk with the same key as `false_accept_bench` (shared cache).
struct Synth {
    root: PathBuf,
    cache: PathBuf,
    engines: HashMap<&'static str, sherpa_onnx::OfflineTts>,
}

impl Synth {
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

/// Per 20 ms frame: voiced on the clean voice.
fn voiced_frames(clean: &[f32]) -> Vec<bool> {
    clean
        .chunks(FRAME_MS * MS)
        .map(|f| {
            let s: f32 = f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32;
            10.0 * s.max(1e-12).log10() > VOICED_DB
        })
        .collect()
}

/// Clean enrollment (3000 ms from the voiced onset) and clean talk of one speaker.
struct Spk {
    label: String,
    enroll: Vec<f32>,
    talk: Vec<f32>,
}

fn enroll_clip(clean: &[f32]) -> Vec<f32> {
    let onset = voiced_frames(clean).iter().position(|v| *v).unwrap_or(0) * FRAME_MS * MS;
    clean[onset..(onset + ENROLL_MS * MS).min(clean.len())].to_vec()
}

fn piper_speakers(synth: &mut Synth) -> Result<Vec<Spk>, String> {
    let mut voices = vec![AMY];
    voices.extend_from_slice(&TEST_VOICES);
    voices.extend_from_slice(&TUNE_VOICES);
    let long = lines(LONG);
    let mut out = Vec::new();
    for v in &voices {
        eprintln!("piper voice {}", v.name);
        let enroll = enroll_clip(&synth.concat(v, &long[0..2])?);
        let mut texts: Vec<&str> = lines(LONG_BACKGROUND);
        texts.extend_from_slice(&long[8..16]);
        let label = if v.bundle.contains("libritts") {
            format!("{} {}", v.name.trim_start_matches("en_US-"), v.sid)
        } else {
            v.name.trim_start_matches("en_US-").to_string()
        };
        out.push(Spk {
            label,
            enroll,
            talk: synth.concat(v, &texts)?,
        });
    }
    Ok(out)
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    Ok(out)
}

/// Same-session LibriSpeech speakers of `test-clean`: the chapter with the most audio is the call.
fn libri_speakers(root: &Path) -> Result<Vec<Spk>, String> {
    let mut out = Vec::new();
    for spk in sorted_dirs(&root.join("test-clean"))? {
        let id = spk
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut best: Option<(u64, Vec<PathBuf>)> = None;
        for ch in sorted_dirs(&spk)? {
            let mut files: Vec<PathBuf> = fs::read_dir(&ch)
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "wav"))
                .collect();
            files.sort();
            let size: u64 = files
                .iter()
                .filter_map(|p| fs::metadata(p).ok())
                .map(|m| m.len())
                .sum();
            if best.as_ref().is_none_or(|(s, _)| size > *s) {
                best = Some((size, files));
            }
        }
        let files = best.ok_or(format!("{id}: no chapter"))?.1;
        if files.is_empty() {
            return Err(format!("test-clean/{id}: no decoded wav files"));
        }
        let mut raw = Vec::new();
        for f in &files {
            if raw.len() >= (LIBRI_STREAM_S + 5) * SR as usize {
                break;
            }
            raw.extend(read_wav(f)?);
            raw.extend(std::iter::repeat_n(0.0f32, GAP_MS * MS));
        }
        let all = normalize_to(&raw, MAIN_DB);
        let onset = voiced_frames(&all).iter().position(|v| *v).unwrap_or(0) * FRAME_MS * MS;
        let cut = (onset + ENROLL_MS * MS).min(all.len());
        out.push(Spk {
            label: id,
            enroll: all[onset..cut].to_vec(),
            talk: all[cut..].to_vec(),
        });
    }
    Ok(out)
}

/// Sample range of a span from voiced frame `vi` (index into the voiced-frame list) until
/// `len_ms` voiced ms, with `pre_ms` of whatever precedes it.
fn span(voiced: &[usize], vi: usize, len_ms: usize, pre_ms: usize) -> Option<(usize, usize)> {
    let need = len_ms / FRAME_MS;
    let last = *voiced.get(vi + need - 1)?;
    let start = (voiced[vi] * FRAME_MS).saturating_sub(pre_ms);
    Some((start * MS, (last + 1) * FRAME_MS * MS))
}

/// Short audio per (shape, length, position) of one talk stream, cut from `audio` (clean or bed)
/// at points chosen on the clean voice.
fn shorts(clean: &[f32], audio: &[f32]) -> Vec<Vec<Vec<Option<Vec<f32>>>>> {
    let voiced: Vec<usize> = voiced_frames(clean)
        .iter()
        .enumerate()
        .filter(|(_, v)| **v)
        .map(|(i, _)| i)
        .collect();
    let max_need = LENGTHS[LENGTHS.len() - 1] / FRAME_MS;
    let room = voiced.len().saturating_sub(max_need + 1);
    let point = |p: usize| p * room / POSITIONS;
    SHAPES
        .iter()
        .map(|&segs| {
            LENGTHS
                .iter()
                .map(|&len| {
                    (0..POSITIONS)
                        .map(|p| {
                            let mut out = Vec::new();
                            for s in 0..segs {
                                let vi = point((p + s * POSITIONS / 2) % POSITIONS);
                                let pre = if s == 0 { PRE_ROLL_MS } else { 0 };
                                let (a, b) = span(&voiced, vi, len / segs, pre)?;
                                out.extend_from_slice(&audio[a..b.min(audio.len())]);
                            }
                            Some(out)
                        })
                        .collect()
                })
                .collect()
        })
        .collect()
}

/// Embeddings of one speaker in one condition: enrollment and [shape][length][position].
struct SpkEmb {
    enroll: Vec<f32>,
    shorts: Vec<Vec<Vec<Option<Vec<f32>>>>>,
}

fn embed_speaker(
    m: &mut Embedder,
    sp: &Spk,
    index: usize,
    bed: Option<(&[f32], f32)>,
) -> Result<SpkEmb, String> {
    let (enroll, talk) = match bed {
        Some((b, db)) => (
            with_bed(&sp.enroll, b, index * 7 * SR as usize, db),
            with_bed(&sp.talk, b, index * 7 * SR as usize + 31 * SR as usize, db),
        ),
        None => (sp.enroll.clone(), sp.talk.clone()),
    };
    let enroll = m
        .embed(&enroll)
        .ok_or(format!("{}: enrollment not ready", sp.label))?;
    let clips = shorts(&sp.talk, &talk);
    let shorts = clips
        .iter()
        .map(|by_len| {
            by_len
                .iter()
                .map(|by_pos| {
                    by_pos
                        .iter()
                        .map(|c| c.as_ref().and_then(|c| m.embed(c)))
                        .collect()
                })
                .collect()
        })
        .collect();
    Ok(SpkEmb { enroll, shorts })
}

#[derive(Default)]
struct Cell {
    caller: Vec<f32>,
    other: Vec<f32>,
    /// Short audio the embedder could not score (no split either way).
    unscored: usize,
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "-".into()
    } else {
        format!("{:.1}", 100.0 * n as f64 / d as f64)
    }
}

fn quantile(v: &[f32], p: f64) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[((s.len() - 1) as f64 * p).round() as usize]
}

struct Args {
    voices_root: PathBuf,
    speaker_dir: PathBuf,
    bed: PathBuf,
    cache: PathBuf,
    librispeech: PathBuf,
    bed_db: f32,
    out: Option<PathBuf>,
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
        librispeech: get("--librispeech")?.into(),
        bed_db: map
            .get("--bed-db")
            .map(|s| s.parse::<f32>().map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or(-12.0),
        out: map.get("--out").map(PathBuf::from),
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

/// [corpus][cond][shape][length] cells, plus per-pair other scores for the Piper incident pair.
fn run() -> Result<(), String> {
    let args = parse_args()?;
    let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
    let mut m = Embedder::open(&args.speaker_dir.join("nemo_en_titanet_small.onnx"))?;
    fs::create_dir_all(&args.cache).map_err(|e| e.to_string())?;
    let mut synth = Synth {
        root: args.voices_root.clone(),
        cache: args.cache.clone(),
        engines: HashMap::new(),
    };
    let corpora: Vec<(&str, Vec<Spk>)> = vec![
        ("Piper", piper_speakers(&mut synth)?),
        ("LibriSpeech test-clean", libri_speakers(&args.librispeech)?),
    ];
    let conds: [(&str, bool); 2] = [("bed", true), ("clean", false)];

    let mut md = String::new();
    let _ = writeln!(
        md,
        "Generated by `short_rejoin_bench` (TitaNet-small raw cosine, gate threshold {GATE_THRESHOLD}, \
         bed {} dB, {POSITIONS} start points per speaker).\n",
        args.bed_db
    );
    // [bound][length] -> (caller splits, caller n) over every corpus, condition and shape.
    let mut all_caller: Vec<Vec<(usize, usize)>> = vec![vec![(0, 0); LENGTHS.len()]; BOUNDS.len()];
    let mut min_caller: Vec<f32> = vec![f32::INFINITY; LENGTHS.len()];
    let mut incident = String::new();
    for (corpus, spks) in &corpora {
        for (cname, has_bed) in conds {
            eprintln!("{corpus} {cname}: embedding {} speakers", spks.len());
            let embs: Vec<SpkEmb> = spks
                .iter()
                .enumerate()
                .map(|(i, sp)| {
                    embed_speaker(&mut m, sp, i, has_bed.then_some((&bed[..], args.bed_db)))
                })
                .collect::<Result<_, _>>()?;
            let _ = writeln!(
                md,
                "## {corpus}, {cname} ({} speakers)\n\n| shape | voiced ms | n caller / other | unscored | caller min / p1 | other p50 / p90 | other scores caller % |{}",
                spks.len(),
                BOUNDS.iter().map(|b| format!(" split @<{b:.2} caller / other % |")).collect::<String>()
            );
            let _ = writeln!(
                md,
                "|---|---|---|---|---|---|---|{}",
                "---|".repeat(BOUNDS.len())
            );
            for (si, segs) in SHAPES.iter().enumerate() {
                for (li, len) in LENGTHS.iter().enumerate() {
                    let mut cell = Cell::default();
                    for (ci, c) in embs.iter().enumerate() {
                        for (oi, o) in embs.iter().enumerate() {
                            for e in &o.shorts[si][li] {
                                match e {
                                    None => cell.unscored += 1,
                                    Some(e) => {
                                        let s = cosine(&c.enroll, e);
                                        if ci == oi {
                                            cell.caller.push(s);
                                        } else {
                                            cell.other.push(s);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let cmin = quantile(&cell.caller, 0.0);
                    min_caller[li] = min_caller[li].min(cmin);
                    let mut row = format!(
                        "| {segs} seg | {len} | {} / {} | {} | {cmin:.3} / {:.3} | {:.3} / {:.3} | {} |",
                        cell.caller.len(),
                        cell.other.len(),
                        cell.unscored,
                        quantile(&cell.caller, 0.01),
                        quantile(&cell.other, 0.5),
                        quantile(&cell.other, 0.9),
                        pct(
                            cell.other.iter().filter(|s| **s >= GATE_THRESHOLD).count(),
                            cell.other.len()
                        ),
                    );
                    for (bi, b) in BOUNDS.iter().enumerate() {
                        let cs = cell.caller.iter().filter(|s| **s < *b).count();
                        let os = cell.other.iter().filter(|s| **s < *b).count();
                        all_caller[bi][li].0 += cs;
                        all_caller[bi][li].1 += cell.caller.len();
                        let _ = write!(
                            row,
                            " {} / {} |",
                            pct(cs, cell.caller.len()),
                            pct(os, cell.other.len())
                        );
                    }
                    let _ = writeln!(md, "{row}");
                }
            }
            md.push('\n');
            if *corpus == "Piper" {
                // The incident pair: caller amy-low, bystander hfc_female-medium.
                let ci = spks.iter().position(|s| s.label == "amy-low");
                let oi = spks.iter().position(|s| s.label == "hfc_female-medium");
                if let (Some(ci), Some(oi)) = (ci, oi) {
                    let _ = write!(incident, "| {cname} |");
                    for (si, segs) in SHAPES.iter().enumerate() {
                        for (li, len) in LENGTHS.iter().enumerate() {
                            let v: Vec<f32> = embs[oi].shorts[si][li]
                                .iter()
                                .flatten()
                                .map(|e| cosine(&embs[ci].enroll, e))
                                .collect();
                            let below = |b: f32| pct(v.iter().filter(|s| **s < b).count(), v.len());
                            let _ = write!(
                                incident,
                                " {segs}s/{len}: {} / {} / {} max {:.3} |",
                                below(0.10),
                                below(0.15),
                                below(0.20),
                                quantile(&v, 1.0)
                            );
                        }
                    }
                    incident.push('\n');
                }
            }
        }
    }
    let _ = writeln!(
        md,
        "## Paused caller over all corpora, conditions and shapes\n\nCaller splits (own short audio below the bound) / caller samples, per voiced length.\n\n| bound |{}",
        LENGTHS.iter().map(|l| format!(" {l} ms |")).collect::<String>()
    );
    let _ = writeln!(md, "|---|{}", "---|".repeat(LENGTHS.len()));
    for (bi, b) in BOUNDS.iter().enumerate() {
        let _ = writeln!(
            md,
            "| < {b:.2} |{}",
            all_caller[bi]
                .iter()
                .map(|(n, d)| format!(" {n} / {d} |"))
                .collect::<String>()
        );
    }
    let _ = writeln!(
        md,
        "| min caller score |{}\n",
        min_caller
            .iter()
            .map(|s| format!(" {s:.3} |"))
            .collect::<String>()
    );
    let _ = writeln!(
        md,
        "## Incident pair (Piper, caller amy-low, bystander hfc_female-medium): split % at < 0.10 / 0.15 / 0.20 and max score\n\n{incident}"
    );
    let mut t = m.times.clone();
    t.sort_by(f64::total_cmp);
    if !t.is_empty() {
        let _ = writeln!(
            md,
            "Embed time per short clip (one thread): p50 {:.1} ms, p99 {:.1} ms, n {}.",
            t[t.len() / 2],
            t[((t.len() - 1) as f64 * 0.99) as usize],
            t.len()
        );
    }
    match &args.out {
        Some(p) => fs::write(p, &md).map_err(|e| format!("write {}: {e}", p.display()))?,
        None => print!("{md}"),
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("short_rejoin_bench: {e}");
        std::process::exit(1);
    }
}
