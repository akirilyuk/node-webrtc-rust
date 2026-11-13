//! Adaptive speaker reference bench (plan `2026-11-13-speaker-split-other-to-caller`, chunk 4,
//! decision e): does adapting the reference from confident caller windows lower caller FRR over a
//! long call whose channel drifts, without raising the false-accept rate anywhere, including a
//! poison set of the bystanders closest to the caller? It decides the default of
//! `speakerGate.adaptReference`. Results: `ADAPT_REFERENCE_BENCH.md`.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example adapt_reference_bench -- \
//!   --model <.models/speaker/nemo_en_titanet_small.onnx> --librispeech <LibriSpeech dir> \
//!   --bed <tocumen48.f32> [--bed-db -12] [--out <file.md>]
//! ```
//!
//! Real speech only (LibriSpeech `test-clean`, CC BY 4.0, flac decoded to wav next to the flac
//! files). Audio and embeddings stay outside the repo; nothing is written but the report.
//!
//! - Session per caller (40 speakers): the chapter with the most audio is the call, at -20 dBFS.
//!   Enrollment = its first 3000 ms from the voiced onset (the gate's `enroll_speech_ms`). Every
//!   later file of the chapter (up to `SESSION_S`) is one caller utterance.
//! - Windows: each utterance is tiled in 1.5 s windows, each clip with the gate's 300 ms
//!   pre-roll; windows less than half voiced are skipped. Threshold 0.459 (today's gate).
//! - After each caller utterance one bystander utterance follows (a file of another speaker),
//!   rotating over 3 bystanders. Sets:
//!   - `clean`: bystanders are the next 3 speakers in list order; no channel change.
//!   - `drift`: the same, with a channel that drifts over the call (by caller utterance index):
//!     first third unchanged, second third telephone band (300-3400 Hz) at -6 dB, last third
//!     telephone band at -12 dB through an 8-bit mu-law round trip. Enrollment is unchanged
//!     (clean), bystanders share the call's channel at their slot.
//!   - `bed`: the Tocumen airport bed at `--bed-db` re the voice, under enrollment and stream.
//!   - `poison`: `bed`, with the 3 bystanders whose clean windows score highest against the
//!     caller's enrollment (the nearest voices, many windows near the threshold).
//! - Fixed reference: the enrollment embedding. Adapted: the gate's rule, with the gate's own
//!   [`adapt_step`]: a window scoring >= threshold + `ADAPT_MARGIN` is a candidate; an utterance
//!   commits only when all its windows score `caller` (any utterance, so a bystander utterance
//!   that is falsely accepted end to end poisons the reference, as it would live); EMA weight
//!   `ADAPT_ALPHA`; rejected below `ANCHOR_MIN_COSINE` against the enrollment.
//! - FRR = caller windows scored below the threshold; FAR = bystander windows scored at or above
//!   it, each against the reference current at that point of the call.

use std::fs;
use std::path::{Path, PathBuf};

use node_webrtc_rust_speech::speaker_gate::{adapt_step, AdaptStep, ADAPT_MARGIN};
use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

mod support;

use support::{db_gain, read_wav, resample_linear, rms_voiced_db};

const SR: i32 = 16_000;
const MS: usize = 16;
const PRE_ROLL_MS: usize = 300;
const ENROLL_MS: usize = 3000;
const WINDOW_MS: usize = 1500;
const THRESHOLD: f32 = 0.459;
const MAIN_DB: f32 = -20.0;
const SESSION_S: usize = 300;
const BYSTANDERS: usize = 3;
/// Bystander utterances (files) embedded per speaker.
const BYSTANDER_FILES: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chan {
    Clean,
    /// Telephone band, -6 dB.
    Tel,
    /// Telephone band, -12 dB, 8-bit mu-law.
    TelMu,
    Bed,
}

const CHANS: [Chan; 4] = [Chan::Clean, Chan::Tel, Chan::TelMu, Chan::Bed];

struct Embedder(SpeakerEmbeddingExtractor);

impl Embedder {
    fn embed(&self, samples: &[f32]) -> Option<Vec<f32>> {
        let stream = self.0.create_stream()?;
        stream.accept_waveform(SR, samples);
        stream.input_finished();
        if !self.0.is_ready(&stream) {
            return None;
        }
        self.0.compute(&stream)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut d, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        d += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    (d / (na.sqrt() * nb.sqrt())) as f32
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

/// One-pole filters: high-pass at 300 Hz, then two low-pass stages at 3400 Hz.
fn telephone(x: &[f32]) -> Vec<f32> {
    let dt = 1.0 / SR as f32;
    let rc_hp = 1.0 / (2.0 * std::f32::consts::PI * 300.0);
    let a_hp = rc_hp / (rc_hp + dt);
    let rc_lp = 1.0 / (2.0 * std::f32::consts::PI * 3400.0);
    let a_lp = dt / (rc_lp + dt);
    let mut out = Vec::with_capacity(x.len());
    let (mut prev_x, mut prev_y, mut l1, mut l2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &s in x {
        let hp = a_hp * (prev_y + s - prev_x);
        prev_x = s;
        prev_y = hp;
        l1 += a_lp * (hp - l1);
        l2 += a_lp * (l1 - l2);
        out.push(l2);
    }
    out
}

fn mu_law_8bit(x: &[f32]) -> Vec<f32> {
    let mu = 255.0f32;
    x.iter()
        .map(|&s| {
            let s = s.clamp(-1.0, 1.0);
            let y = s.signum() * (1.0 + mu * s.abs()).ln() / (1.0 + mu).ln();
            let q = (y * 127.0).round() / 127.0;
            q.signum() * ((1.0 + mu).powf(q.abs()) - 1.0) / mu
        })
        .collect()
}

fn with_bed(voice: &[f32], bed: &[f32], offset: usize, bed_db: f32) -> Vec<f32> {
    let bed_rms = {
        let s: f64 = bed.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
        (s / bed.len() as f64).sqrt() as f32
    };
    let target = db_gain(rms_voiced_db(voice, SR) + bed_db);
    let gain = if bed_rms > 0.0 { target / bed_rms } else { 0.0 };
    voice
        .iter()
        .enumerate()
        .map(|(i, x)| x + bed[(offset + i) % bed.len()] * gain)
        .collect()
}

fn channel(x: &[f32], chan: Chan, bed: &[f32], bed_db: f32, offset: usize) -> Vec<f32> {
    match chan {
        Chan::Clean => x.to_vec(),
        Chan::Tel => telephone(x).iter().map(|s| s * db_gain(-6.0)).collect(),
        Chan::TelMu => {
            let t: Vec<f32> = telephone(x).iter().map(|s| s * db_gain(-12.0)).collect();
            mu_law_8bit(&t)
        }
        Chan::Bed => with_bed(x, bed, offset, bed_db),
    }
}

/// Window embeddings of one utterance (file audio at -20 dBFS gain of the chapter).
fn utterance_windows(
    emb: &Embedder,
    audio: &[f32],
    chan: Chan,
    bed: &[f32],
    bed_db: f32,
    offset: usize,
) -> Vec<Vec<f32>> {
    let x = channel(audio, chan, bed, bed_db, offset);
    let w = WINDOW_MS * MS;
    let pre = PRE_ROLL_MS * MS;
    let mut out = Vec::new();
    let mut start = 0;
    while start + w <= x.len() {
        if voiced_share(&audio[start..start + w]) >= 0.5 {
            let clip = &x[start.saturating_sub(pre)..start + w];
            if let Some(e) = emb.embed(clip) {
                out.push(e);
            }
        }
        start += w;
    }
    out
}

struct Speaker {
    id: String,
    /// Files of the session chapter, sorted.
    files: Vec<PathBuf>,
}

fn sorted(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    v.sort();
    Ok(v)
}

fn speakers(root: &Path) -> Result<Vec<Speaker>, String> {
    let mut out = Vec::new();
    for spk in sorted(&root.join("test-clean"))? {
        if !spk.is_dir() {
            continue;
        }
        let mut best: Option<(u64, Vec<PathBuf>)> = None;
        for ch in sorted(&spk)? {
            if !ch.is_dir() {
                continue;
            }
            let files: Vec<PathBuf> = sorted(&ch)?
                .into_iter()
                .filter(|p| p.extension().is_some_and(|x| x == "wav"))
                .collect();
            let size: u64 = files
                .iter()
                .filter_map(|p| fs::metadata(p).ok())
                .map(|m| m.len())
                .sum();
            if best.as_ref().is_none_or(|(s, _)| size > *s) {
                best = Some((size, files));
            }
        }
        let Some((_, files)) = best.filter(|(_, f)| !f.is_empty()) else {
            return Err(format!("{}: no decoded wav files", spk.display()));
        };
        out.push(Speaker {
            id: spk
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            files,
        });
    }
    Ok(out)
}

/// One speaker's call: enrollment per channel (clean, bed) and utterance windows per channel.
struct Call {
    enroll_clean: Vec<f32>,
    enroll_bed: Vec<f32>,
    /// [chan][utterance] -> windows
    utts: Vec<Vec<Vec<Vec<f32>>>>,
}

fn load_call(emb: &Embedder, sp: &Speaker, bed: &[f32], bed_db: f32) -> Result<Call, String> {
    let mut audio: Vec<Vec<f32>> = Vec::new();
    let mut total = 0;
    for f in &sp.files {
        if total >= (SESSION_S + 10) * SR as usize {
            break;
        }
        let a = read_wav(f)?;
        total += a.len();
        audio.push(a);
    }
    // One gain for the whole chapter, so level differences between files stay.
    let joined: Vec<f32> = audio.iter().flatten().copied().collect();
    let gain = db_gain(MAIN_DB - rms_voiced_db(&joined, SR));
    let audio: Vec<Vec<f32>> = audio
        .into_iter()
        .map(|a| a.iter().map(|x| x * gain).collect())
        .collect();
    let first = &audio[0];
    let onset = first
        .chunks(20 * MS)
        .position(|f| voiced_share(f) > 0.0)
        .unwrap_or(0)
        * 20
        * MS;
    let enroll = &first[onset..(onset + ENROLL_MS * MS).min(first.len())];
    let enroll_clean = emb.embed(enroll).ok_or("enroll embed failed")?;
    let enroll_bed = emb
        .embed(&with_bed(enroll, bed, 0, bed_db))
        .ok_or("enroll embed failed")?;
    let mut utts = vec![Vec::new(); CHANS.len()];
    for (ci, chan) in CHANS.iter().enumerate() {
        let mut offset = 0;
        for a in &audio[1..] {
            utts[ci].push(utterance_windows(emb, a, *chan, bed, bed_db, offset));
            offset += a.len();
        }
    }
    Ok(Call {
        enroll_clean,
        enroll_bed,
        utts,
    })
}

#[derive(Default, Clone, Copy)]
struct Tally {
    caller: usize,
    caller_rej: [usize; 2],
    by: usize,
    by_acc: [usize; 2],
    commits: usize,
    rejected: usize,
    by_commits: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Clean,
    Drift,
    Bed,
    Poison,
}

impl Set {
    fn name(self) -> &'static str {
        match self {
            Set::Clean => "clean",
            Set::Drift => "drift",
            Set::Bed => "bed",
            Set::Poison => "poison",
        }
    }
}

const SETS: [Set; 4] = [Set::Clean, Set::Drift, Set::Bed, Set::Poison];

fn ci(c: Chan) -> usize {
    CHANS.iter().position(|x| *x == c).unwrap_or(0)
}

/// Simulates one call; returns the tally and the cosine-to-anchor trace (min, final).
fn simulate(calls: &[Call], c: usize, bys: &[usize], set: Set, tally: &mut Tally) -> (f32, f32) {
    let call = &calls[c];
    let anchor = if matches!(set, Set::Bed | Set::Poison) {
        call.enroll_bed.clone()
    } else {
        call.enroll_clean.clone()
    };
    let mut adapted = anchor.clone();
    let n = call.utts[0].len();
    let mut min_cos = 1.0f32;
    let mut by_file = vec![0usize; bys.len()];
    for u in 0..n {
        let chan = match set {
            Set::Clean => Chan::Clean,
            Set::Bed | Set::Poison => Chan::Bed,
            Set::Drift => match (u * 3) / n.max(1) {
                0 => Chan::Clean,
                1 => Chan::Tel,
                _ => Chan::TelMu,
            },
        };
        // Caller utterance, then one bystander utterance.
        let bi = u % bys.len();
        let b = bys[bi];
        let bfiles = &calls[b].utts[ci(chan)];
        let bwins: &[Vec<f32>] = if bfiles.is_empty() {
            &[]
        } else {
            let f = by_file[bi] % bfiles.len().min(BYSTANDER_FILES);
            by_file[bi] += 1;
            &bfiles[f]
        };
        for (is_caller, wins) in [(true, &call.utts[ci(chan)][u][..]), (false, bwins)] {
            if wins.is_empty() {
                continue;
            }
            let mut all_caller = true;
            let mut cands: Vec<Vec<f32>> = Vec::new();
            for w in wins {
                let fixed = cosine(&anchor, w);
                let ad = cosine(&adapted, w);
                if is_caller {
                    tally.caller += 1;
                    tally.caller_rej[0] += usize::from(fixed < THRESHOLD);
                    tally.caller_rej[1] += usize::from(ad < THRESHOLD);
                } else {
                    tally.by += 1;
                    tally.by_acc[0] += usize::from(fixed >= THRESHOLD);
                    tally.by_acc[1] += usize::from(ad >= THRESHOLD);
                }
                if ad < THRESHOLD {
                    all_caller = false;
                } else if ad >= THRESHOLD + ADAPT_MARGIN {
                    cands.push(w.clone());
                }
            }
            if all_caller && !cands.is_empty() {
                match adapt_step(&adapted, &anchor, &cands) {
                    AdaptStep::Commit {
                        reference,
                        cos_to_anchor,
                    } => {
                        adapted = reference;
                        tally.commits += 1;
                        tally.by_commits += usize::from(!is_caller);
                        min_cos = min_cos.min(cos_to_anchor);
                    }
                    AdaptStep::RejectAnchor { .. } => tally.rejected += 1,
                    AdaptStep::Skip => {}
                }
            }
        }
    }
    (min_cos, cosine(&adapted, &anchor))
}

fn pct(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

fn run() -> Result<(), String> {
    let mut model = None;
    let mut root = None;
    let mut bed_path = None;
    let mut bed_db = -12.0f32;
    let mut out = None;
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let v = it.next().ok_or(format!("{k} needs a value"))?;
        match k.as_str() {
            "--model" => model = Some(PathBuf::from(v)),
            "--librispeech" => root = Some(PathBuf::from(v)),
            "--bed" => bed_path = Some(PathBuf::from(v)),
            "--bed-db" => bed_db = v.parse().map_err(|e| format!("--bed-db: {e}"))?,
            "--out" => out = Some(PathBuf::from(v)),
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    let model = model.ok_or("missing --model")?;
    let root = root.ok_or("missing --librispeech")?;
    let bed_path = bed_path.ok_or("missing --bed")?;
    let bytes = fs::read(&bed_path).map_err(|e| format!("read {}: {e}", bed_path.display()))?;
    let bed48: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let bed = resample_linear(&bed48, 48_000, SR);
    let config = SpeakerEmbeddingExtractorConfig {
        model: Some(model.to_string_lossy().into_owned()),
        num_threads: 2,
        ..Default::default()
    };
    let emb = Embedder(
        SpeakerEmbeddingExtractor::create(&config).ok_or("cannot open the speaker model")?,
    );
    let sps = speakers(&root)?;
    let started = std::time::Instant::now();
    let mut calls = Vec::new();
    for (i, sp) in sps.iter().enumerate() {
        calls.push(load_call(&emb, sp, &bed, bed_db)?);
        eprintln!(
            "[adapt-bench] loaded {}/{} ({}) after {:.0} s",
            i + 1,
            sps.len(),
            sp.id,
            started.elapsed().as_secs_f64()
        );
    }
    let n = calls.len();
    let mut rows = Vec::new();
    let mut per_set = Vec::new();
    for set in SETS {
        let mut tally = Tally::default();
        let mut mins = Vec::new();
        let mut finals = Vec::new();
        for c in 0..n {
            let bys: Vec<usize> = if set == Set::Poison {
                // Nearest voices: mean clean-window score against the caller's bed enrollment.
                let mut s: Vec<(f32, usize)> = (0..n)
                    .filter(|&b| b != c)
                    .map(|b| {
                        let wins: Vec<&Vec<f32>> =
                            calls[b].utts[ci(Chan::Bed)].iter().flatten().collect();
                        let m = wins
                            .iter()
                            .map(|w| cosine(&calls[c].enroll_bed, w))
                            .sum::<f32>()
                            / wins.len().max(1) as f32;
                        (m, b)
                    })
                    .collect();
                s.sort_by(|a, b| b.0.total_cmp(&a.0));
                s.iter().take(BYSTANDERS).map(|x| x.1).collect()
            } else {
                (1..=BYSTANDERS).map(|k| (c + k) % n).collect()
            };
            let (min_cos, final_cos) = simulate(&calls, c, &bys, set, &mut tally);
            mins.push(min_cos);
            finals.push(final_cos);
        }
        mins.sort_by(f32::total_cmp);
        finals.sort_by(f32::total_cmp);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
        rows.push(format!(
            "| {} | {} | {:.2} | {:.2} | {} | {:.3} | {:.3} | {} / {} / {} | {:.3} | {:.3} |",
            set.name(),
            tally.caller,
            pct(tally.caller_rej[0], tally.caller),
            pct(tally.caller_rej[1], tally.caller),
            tally.by,
            pct(tally.by_acc[0], tally.by),
            pct(tally.by_acc[1], tally.by),
            tally.commits,
            tally.by_commits,
            tally.rejected,
            mean(&finals),
            mins.first().copied().unwrap_or(1.0),
        ));
        per_set.push((set, tally));
    }
    let frr_drops = per_set
        .iter()
        .any(|(s, t)| *s == Set::Drift && t.caller_rej[1] < t.caller_rej[0]);
    let far_holds = per_set.iter().all(|(_, t)| t.by_acc[1] <= t.by_acc[0]);
    let pass = frr_drops && far_holds;
    let mut md = String::new();
    md.push_str(&format!(
        "Speakers: {n} (LibriSpeech test-clean, same-session call, up to {SESSION_S} s), \
         1.5 s windows + 300 ms pre-roll, threshold {THRESHOLD}, bed {bed_db} dB. \
         Run time {:.0} s.\n\n",
        started.elapsed().as_secs_f64()
    ));
    md.push_str(
        "| set | caller windows | FRR fixed % | FRR adapted % | bystander windows | FAR fixed % | FAR adapted % | commits / by bystander / rejected (anchor) | mean final cos to anchor | min cos to anchor |\n\
         |---|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in &rows {
        md.push_str(r);
        md.push('\n');
    }
    md.push_str(&format!(
        "\nGate: FRR drops on drift: {frr_drops}; FAR does not rise on any set: {far_holds}. \
         Default adaptReference: {}.\n",
        if pass { "on" } else { "off" }
    ));
    println!("{md}");
    if let Some(out) = out {
        fs::write(&out, &md).map_err(|e| format!("write {}: {e}", out.display()))?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("adapt_reference_bench: {e}");
        std::process::exit(1);
    }
}
