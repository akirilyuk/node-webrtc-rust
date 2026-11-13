//! Speaker gate false-accept bench per voice pair (plan `2026-11-13-speaker-split-other-to-caller`,
//! chunk 1b, decision g): how often does a window of another voice score `caller` against an
//! enrolled caller, per (caller voice, other voice) pair, and which gate change cuts that without
//! failing real caller windows? Measurement only; the gate is not changed here.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example false_accept_bench -- \
//!   --voices-root <.models> --speaker-dir <.models/speaker> --bed <tocumen48.f32> \
//!   --cache <dir> --airport-a <scene A 16 kHz mono wav> --airport-b <scene B 16 kHz mono wav> \
//!   [--bed-db -12] [--out <file.md>]
//! ```
//!
//! - Voices: callers and others are the test set (`en_US-amy-low` sid 0 plus `TEST_VOICES`); the
//!   tune set (`TUNE_VOICES`) only picks thresholds. The impostor cohort for score normalization
//!   is 30 `en_US-libritts_r-medium` speakers that are in neither set ([`COHORT_SIDS`]).
//! - Per voice: an enrollment clip (300 ms pre-roll + 3000 ms, the gate's `enroll_speech_ms`) from
//!   two long sentences, and a talk stream of 20 other sentences (200 ms gaps). Voices are
//!   normalized to -20 dBFS voiced RMS. Condition `bed`: the Tocumen airport bed (48 kHz mono f32le,
//!   resampled) at `--bed-db` re the voice under both enrollment and stream; condition `clean`: none.
//! - Windows: the stream is tiled in consecutive windows of `W` s (1.5 / 2.0 / 2.5 / 3.0), each
//!   clip with the gate's 300 ms pre-roll; windows less than half voiced (20 ms frames above -45
//!   dBFS on the clean voice) are skipped. A window is `caller` when its score >= the threshold.
//! - FAR(pair c, o) = share of o's windows accepted against c's enrollment; FRR(c) = share of c's
//!   own windows rejected. Same condition for enrollment and windows.
//! - Scores: `raw` cosine; `s-norm` = mean of the z-scores of the raw score against the cohort
//!   scores of the enrollment and of the window; `as-norm` the same over the top 10 cohort scores.
//! - Thresholds: today's gate is TitaNet-small raw at 0.459, also kept for longer TitaNet windows.
//!   Every other candidate's threshold is picked on the tune set: the highest threshold whose tune
//!   FRR is at most max(today's tune FRR, 1 %).
//! - Airport demo check: the gate's enrollment (scene 1.7-5.0 s) and its window grid for the two
//!   leaking utterances. The utterance start is fitted to the logged TitaNet 1.5 s scores
//!   ([`AIRPORT`]); each candidate then scores the clip of `W` s (+ pre-roll) that ends where the
//!   logged caller window ends, i.e. the window that would confirm at the same moment.
//! - Embed time: p50 / p99 per window per model and window length (one thread).

use std::collections::HashMap;
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
const PRE_ROLL_MS: usize = 300;
const ENROLL_MS: usize = 3000;
const GATE_THRESHOLD: f32 = 0.459;
const MAIN_DB: f32 = -20.0;
const WINDOWS_S: [f32; 4] = [1.5, 2.0, 2.5, 3.0];
const TOP_K: usize = 10;
const GAP_MS: usize = 200;

const LONG: &str = include_str!("support/long_sentences.txt");
const LONG_BACKGROUND: &str = include_str!("support/long_background_sentences.txt");
const BACKGROUND: &str = include_str!("support/background_sentences.txt");

const AMY: Voice = voice("en_US-amy-low", "vits-piper-en_US-amy-low", 0);
const COHORT_SIDS: [i32; 30] = [
    5, 15, 30, 70, 90, 110, 140, 160, 180, 200, 220, 240, 260, 280, 320, 340, 360, 380, 400, 420,
    450, 500, 550, 600, 650, 700, 750, 800, 850, 900,
];

/// Airport demo utterances that leaked (VOICE_DEBUG logs of the split baselines, 0.18.4):
/// (label, approximate utterance start ms, logged TitaNet 1.5 s window scores, caller windows).
struct Airport {
    label: &'static str,
    approx_u_ms: usize,
    logged: &'static [f32],
    caller: &'static [usize],
}

const AIRPORT: [Airport; 2] = [
    Airport {
        label: "A utt-12 (key 3)",
        approx_u_ms: 22_000,
        logged: &[
            -0.058, 0.001, -0.014, -0.030, -0.076, 0.106, 0.220, 0.179, 0.697, 0.650,
        ],
        caller: &[9, 10],
    },
    Airport {
        label: "B utt-10 (key 4)",
        approx_u_ms: 35_900,
        logged: &[0.082, 0.021, 0.059, 0.040, 0.515, 0.514, 0.562],
        caller: &[5, 6, 7],
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Norm {
    Raw,
    Snorm,
    Asnorm,
}

impl Norm {
    fn name(self) -> &'static str {
        match self {
            Norm::Raw => "raw",
            Norm::Snorm => "s-norm",
            Norm::Asnorm => "as-norm",
        }
    }
}

struct Candidate {
    model: usize,
    window: usize,
    norm: Norm,
    /// Fixed threshold (today's gate); `None` = tune-matched.
    fixed: Option<f32>,
}

struct Embedder {
    name: String,
    extractor: SpeakerEmbeddingExtractor,
    times: HashMap<usize, Vec<f64>>,
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
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            name,
            extractor,
            times: HashMap::new(),
        })
    }

    fn embed(&mut self, samples: &[f32], timing_key: Option<usize>) -> Result<Vec<f32>, String> {
        let start = Instant::now();
        let stream = self
            .extractor
            .create_stream()
            .ok_or("create_stream failed")?;
        stream.accept_waveform(SR, samples);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return Err(format!(
                "{}: clip too short ({} samples)",
                self.name,
                samples.len()
            ));
        }
        let e = self.extractor.compute(&stream).ok_or("compute failed")?;
        if let Some(key) = timing_key {
            self.times
                .entry(key)
                .or_default()
                .push(start.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(e)
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

struct Synth {
    root: PathBuf,
    cache: PathBuf,
    engines: HashMap<&'static str, sherpa_onnx::OfflineTts>,
}

impl Synth {
    /// 16 kHz mono clip of `text` in `v`, cached on disk (VITS sampling is random per run).
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

/// `voice` plus the bed (starting at `offset`) at `bed_db` re the voice's voiced RMS.
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

/// Embeddings of one voice in one condition, per model: enrollment and windows per length.
struct VoiceEmb {
    enroll: Vec<Vec<f32>>,
    /// [model][window index] -> embeddings
    windows: Vec<Vec<Vec<Vec<f32>>>>,
}

struct Cond {
    bed: bool,
}

const CONDS: [Cond; 2] = [Cond { bed: true }, Cond { bed: false }];

/// Models embedding each window length: TitaNet (model 0) all lengths, the others 1.5 s only.
fn model_windows(model: usize) -> &'static [usize] {
    if model == 0 {
        &[0, 1, 2, 3]
    } else {
        &[0]
    }
}

#[allow(clippy::too_many_arguments)]
fn embed_voice(
    synth: &mut Synth,
    models: &mut [Embedder],
    v: &Voice,
    index: usize,
    bed: &[f32],
    bed_db: f32,
) -> Result<[VoiceEmb; 2], String> {
    let long = lines(LONG);
    let enroll_clean = synth.concat(v, &long[0..2])?;
    let mut texts: Vec<&str> = lines(LONG_BACKGROUND);
    texts.extend_from_slice(&long[8..16]);
    let stream_clean = synth.concat(v, &texts)?;
    let mut out = Vec::new();
    for cond in &CONDS {
        let offset = index * 7 * SR as usize;
        let (enroll, stream) = if cond.bed {
            (
                with_bed(&enroll_clean, bed, offset, bed_db),
                with_bed(&stream_clean, bed, offset + 3 * SR as usize, bed_db),
            )
        } else {
            (enroll_clean.clone(), stream_clean.clone())
        };
        // The gate's enrollment: speech from the onset (first voiced frame) minus the pre-roll.
        let onset = enroll_clean
            .chunks(20 * MS)
            .position(|f| voiced_share(f) > 0.0)
            .unwrap_or(0)
            * 20
            * MS;
        let e0 = onset.saturating_sub(PRE_ROLL_MS * MS);
        let e1 = (onset + ENROLL_MS * MS).min(enroll.len());
        let mut ve = VoiceEmb {
            enroll: Vec::new(),
            windows: Vec::new(),
        };
        for (m, emb) in models.iter_mut().enumerate() {
            ve.enroll.push(emb.embed(&enroll[e0..e1], None)?);
            let mut per_w = vec![Vec::new(); WINDOWS_S.len()];
            for &wi in model_windows(m) {
                let w = (WINDOWS_S[wi] * 1000.0) as usize * MS;
                let pre = PRE_ROLL_MS * MS;
                let mut start = pre;
                while start + w <= stream.len() {
                    if voiced_share(&stream_clean[start..start + w]) >= 0.5 {
                        per_w[wi].push(emb.embed(&stream[start - pre..start + w], Some(wi))?);
                    }
                    start += w;
                }
            }
            ve.windows.push(per_w);
        }
        out.push(ve);
    }
    let clean = out.pop().ok_or("no clean")?;
    let bedv = out.pop().ok_or("no bed")?;
    Ok([bedv, clean])
}

fn cohort_stats(e: &[f32], cohort: &[Vec<f32>], top: Option<usize>) -> (f32, f32) {
    let mut s: Vec<f32> = cohort.iter().map(|c| cosine(e, c)).collect();
    s.sort_by(|a, b| b.total_cmp(a));
    if let Some(k) = top {
        s.truncate(k);
    }
    let n = s.len() as f32;
    let mean = s.iter().sum::<f32>() / n;
    let var = s.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    (mean, var.sqrt().max(1e-4))
}

fn score(norm: Norm, enroll: &[f32], window: &[f32], cohort: &[Vec<f32>]) -> f32 {
    let raw = cosine(enroll, window);
    let top = match norm {
        Norm::Raw => return raw,
        Norm::Snorm => None,
        Norm::Asnorm => Some(TOP_K),
    };
    let (me, se) = cohort_stats(enroll, cohort, top);
    let (mt, st) = cohort_stats(window, cohort, top);
    0.5 * ((raw - me) / se + (raw - mt) / st)
}

fn pct(n: usize, d: usize) -> String {
    if d == 0 {
        "-".into()
    } else {
        format!("{:.1}", 100.0 * n as f64 / d as f64)
    }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

struct Args {
    voices_root: PathBuf,
    speaker_dir: PathBuf,
    bed: PathBuf,
    cache: PathBuf,
    airport: [PathBuf; 2],
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
        airport: [get("--airport-a")?.into(), get("--airport-b")?.into()],
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

fn clip(audio: &[f32], start_ms: usize, end_ms: usize) -> &[f32] {
    let a = (start_ms * MS).min(audio.len());
    let b = (end_ms * MS).min(audio.len());
    &audio[a..b]
}

/// Airport results per candidate: scores of the caller windows (`None` = could not fit).
struct AirportFit {
    label: &'static str,
    u_ms: usize,
    rmse: f32,
    reproduced: Vec<f32>,
    /// [candidate] -> scores of the caller windows
    scores: Vec<Vec<f32>>,
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    fs::create_dir_all(&args.cache).map_err(|e| e.to_string())?;
    let bed = resample_linear(&read_f32le(&args.bed)?, 48_000, SR);
    let model_files = [
        "nemo_en_titanet_small.onnx",
        "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx",
        "wespeaker_en_voxceleb_resnet34_LM.onnx",
    ];
    let mut models = Vec::new();
    for f in model_files {
        models.push(Embedder::open(&args.speaker_dir.join(f))?);
    }
    let mut synth = Synth {
        root: args.voices_root.clone(),
        cache: args.cache.clone(),
        engines: HashMap::new(),
    };

    let mut test_voices = vec![AMY];
    test_voices.extend_from_slice(&TEST_VOICES);
    let tune_voices: Vec<Voice> = TUNE_VOICES.to_vec();
    let vname = |v: &Voice| {
        let short = v
            .name
            .trim_start_matches("en_US-")
            .trim_end_matches("-medium");
        if v.bundle.contains("libritts") {
            format!("{short} {}", v.sid)
        } else {
            short.to_string()
        }
    };

    let mut test = Vec::new();
    for (i, v) in test_voices.iter().enumerate() {
        eprintln!("test voice {}", vname(v));
        test.push(embed_voice(
            &mut synth,
            &mut models,
            v,
            i,
            &bed,
            args.bed_db,
        )?);
    }
    let mut tune = Vec::new();
    for (i, v) in tune_voices.iter().enumerate() {
        eprintln!("tune voice {}", vname(v));
        tune.push(embed_voice(
            &mut synth,
            &mut models,
            v,
            10 + i,
            &bed,
            args.bed_db,
        )?);
    }
    // Cohort: [cond][model] -> embeddings of a 3.3 s clip per cohort speaker.
    let bg = lines(BACKGROUND);
    let mut cohort: Vec<Vec<Vec<Vec<f32>>>> = vec![vec![Vec::new(); models.len()]; 2];
    for (i, sid) in COHORT_SIDS.iter().enumerate() {
        let v = voice(
            "en_US-libritts_r-medium",
            "vits-piper-en_US-libritts_r-medium",
            *sid,
        );
        let texts = [bg[(2 * i) % bg.len()], bg[(2 * i + 1) % bg.len()]];
        let clean = synth.concat(&v, &texts)?;
        let end = ((PRE_ROLL_MS + ENROLL_MS) * MS).min(clean.len());
        for (ci, cond) in CONDS.iter().enumerate() {
            let a = if cond.bed {
                with_bed(&clean, &bed, (40 + i) * 5 * SR as usize, args.bed_db)
            } else {
                clean.clone()
            };
            for (m, emb) in models.iter_mut().enumerate() {
                cohort[ci][m].push(emb.embed(&a[..end], None)?);
            }
        }
    }

    let mut cands = vec![Candidate {
        model: 0,
        window: 0,
        norm: Norm::Raw,
        fixed: Some(GATE_THRESHOLD),
    }];
    for w in 1..WINDOWS_S.len() {
        cands.push(Candidate {
            model: 0,
            window: w,
            norm: Norm::Raw,
            fixed: Some(GATE_THRESHOLD),
        });
    }
    for w in 1..WINDOWS_S.len() {
        cands.push(Candidate {
            model: 0,
            window: w,
            norm: Norm::Raw,
            fixed: None,
        });
    }
    for w in [0, 1] {
        for norm in [Norm::Snorm, Norm::Asnorm] {
            cands.push(Candidate {
                model: 0,
                window: w,
                norm,
                fixed: None,
            });
        }
    }
    for m in 1..models.len() {
        for norm in [Norm::Raw, Norm::Asnorm] {
            cands.push(Candidate {
                model: m,
                window: 0,
                norm,
                fixed: None,
            });
        }
    }
    let cand_name = |c: &Candidate, models: &[Embedder]| {
        let short = match c.model {
            0 => "titanet",
            1 => "campplus",
            _ => "resnet34",
        };
        let _ = models;
        format!(
            "{short} {}s {}{}",
            WINDOWS_S[c.window],
            c.norm.name(),
            if c.fixed.is_some() { " @0.459" } else { "" }
        )
    };

    // Scores of one set: genuine[cond] (per caller), impostor[cond][caller][other].
    type Scores = (Vec<Vec<Vec<f32>>>, Vec<Vec<Vec<Vec<f32>>>>);
    let set_scores = |set: &[[VoiceEmb; 2]], c: &Candidate| -> Scores {
        let mut gen = vec![vec![Vec::new(); set.len()]; 2];
        let mut imp = vec![vec![vec![Vec::new(); set.len()]; set.len()]; 2];
        for ci in 0..2 {
            for (a, va) in set.iter().enumerate() {
                let e = &va[ci].enroll[c.model];
                for (b, vb) in set.iter().enumerate() {
                    for w in &vb[ci].windows[c.model][c.window] {
                        let s = score(c.norm, e, w, &cohort[ci][c.model]);
                        if a == b {
                            gen[ci][a].push(s);
                        } else {
                            imp[ci][a][b].push(s);
                        }
                    }
                }
            }
        }
        (gen, imp)
    };
    let today_tune = set_scores(&tune, &cands[0]);
    let tune_frr = |gen: &[Vec<Vec<f32>>], t: f32| {
        let all: Vec<f32> = gen.iter().flatten().flatten().copied().collect();
        all.iter().filter(|s| **s < t).count() as f64 / all.len().max(1) as f64
    };
    let target = tune_frr(&today_tune.0, GATE_THRESHOLD).max(0.01);

    // Airport: fit the utterance start on today's gate, then score each candidate.
    let mut airport = Vec::new();
    for (ai, ap) in AIRPORT.iter().enumerate() {
        let audio = read_wav(&args.airport[ai])?;
        let enroll_clip = clip(&audio, 2000 - PRE_ROLL_MS, 2000 + ENROLL_MS);
        let refs: Vec<Vec<f32>> = models
            .iter_mut()
            .map(|m| m.embed(enroll_clip, None))
            .collect::<Result<_, _>>()?;
        let mut best = (f32::MAX, 0usize, Vec::new());
        let mut u = ap.approx_u_ms - 1000;
        while u <= ap.approx_u_ms + 1000 {
            let mut rep = Vec::new();
            for k in 1..=ap.logged.len() {
                let s = u + (k - 1) * 1500;
                let e = models[0].embed(clip(&audio, s - PRE_ROLL_MS, s + 1500), None)?;
                rep.push(cosine(&refs[0], &e));
            }
            let sse: f32 = rep
                .iter()
                .zip(ap.logged)
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
            let rmse = (sse / rep.len() as f32).sqrt();
            if rmse < best.0 {
                best = (rmse, u, rep);
            }
            u += 50;
        }
        let (rmse, u_ms, reproduced) = best;
        let mut scores = Vec::new();
        for c in &cands {
            let mut row = Vec::new();
            for &k in ap.caller {
                let end = u_ms + k * 1500;
                let w = (WINDOWS_S[c.window] * 1000.0) as usize;
                let e = models[c.model].embed(clip(&audio, end - w - PRE_ROLL_MS, end), None)?;
                row.push(score(c.norm, &refs[c.model], &e, &cohort[0][c.model]));
            }
            scores.push(row);
        }
        airport.push(AirportFit {
            label: ap.label,
            u_ms,
            rmse,
            reproduced,
            scores,
        });
    }

    let mut md = String::new();
    md.push_str(&format!(
        "Bed: Tocumen airport CC0 at {} dB re the voice (`bed`), or none (`clean`). Voices at {MAIN_DB} dBFS. \
         Test callers/others: {}. Tune: {}. Cohort: libritts_r sids {:?}. Thresholds: `@0.459` fixed, \
         else tune-matched (tune FRR <= {:.1} %).\n\n",
        args.bed_db,
        test_voices.iter().map(vname).collect::<Vec<_>>().join(", "),
        tune_voices.iter().map(vname).collect::<Vec<_>>().join(", "),
        COHORT_SIDS,
        target * 100.0
    ));

    let mut pair_rows: Vec<Vec<String>> = Vec::new();
    let mut frr_rows: Vec<Vec<String>> = Vec::new();
    md.push_str("### Summary per candidate\n\n| candidate | threshold | test FAR bed / clean % | worst pair FAR bed % | FAR, other = libritts_r bed / clean % | test FRR bed / clean % | tune FAR / FRR % (pooled) | airport A w9, w10 | airport B w5, w6, w7 | airport pass | embed ms p50 / p99 | extra confirm delay s |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for (ci, c) in cands.iter().enumerate() {
        let (tg, ti) = set_scores(&tune, c);
        let thr = match c.fixed {
            Some(t) => t,
            None => {
                let mut all: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
                all.sort_by(f32::total_cmp);
                all[((target * all.len() as f64).floor() as usize).min(all.len() - 1)]
            }
        };
        let (gen, imp) = set_scores(&test, c);
        let count = |v: &[f32], acc: bool| {
            v.iter()
                .filter(|s| if acc { **s >= thr } else { **s < thr })
                .count()
        };
        let mut far_tot = [(0, 0); 2];
        let mut lib_tot = [(0, 0); 2];
        let mut frr_tot = [(0, 0); 2];
        let mut worst = (0.0f64, String::new());
        for a in 0..test.len() {
            for b in 0..test.len() {
                if a == b {
                    continue;
                }
                if ci == 0 {
                    pair_rows.push(vec![format!(
                        "{} ← {}",
                        vname(&test_voices[a]),
                        vname(&test_voices[b])
                    )]);
                }
                let mut cells = Vec::new();
                for k in 0..2 {
                    let v = &imp[k][a][b];
                    let n = count(v, true);
                    far_tot[k].0 += n;
                    far_tot[k].1 += v.len();
                    if test_voices[b].bundle.contains("libritts") {
                        lib_tot[k].0 += n;
                        lib_tot[k].1 += v.len();
                    }
                    if k == 0 {
                        let f = n as f64 / v.len().max(1) as f64;
                        if f > worst.0 {
                            worst = (
                                f,
                                format!("{} ← {}", vname(&test_voices[a]), vname(&test_voices[b])),
                            );
                        }
                    }
                    cells.push(pct(n, v.len()));
                }
                let row = a * (test.len() - 1) + if b > a { b - 1 } else { b };
                pair_rows[row].push(cells.join(" / "));
            }
            if ci == 0 {
                frr_rows.push(vec![vname(&test_voices[a])]);
            }
            let mut cells = Vec::new();
            for k in 0..2 {
                let v = &gen[k][a];
                let n = count(v, false);
                frr_tot[k].0 += n;
                frr_tot[k].1 += v.len();
                cells.push(pct(n, v.len()));
            }
            frr_rows[a].push(cells.join(" / "));
        }
        let tune_far: Vec<f32> = ti.iter().flatten().flatten().flatten().copied().collect();
        let tune_gen: Vec<f32> = tg.iter().flatten().flatten().copied().collect();
        let fmt_air = |s: &[f32]| {
            s.iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let pass = airport
            .iter()
            .all(|a| a.scores[ci].iter().all(|s| *s >= thr));
        let mut times = models[c.model]
            .times
            .get(&c.window)
            .cloned()
            .unwrap_or_default();
        let p50 = percentile(&mut times, 0.5);
        let p99 = percentile(&mut times, 0.99);
        md.push_str(&format!(
            "| {} | {:.3} | {} / {} | {:.1} ({}) | {} / {} | {} / {} | {} / {} | {} | {} | {} | {:.1} / {:.1} | {:.1} |\n",
            cand_name(c, &models),
            thr,
            pct(far_tot[0].0, far_tot[0].1),
            pct(far_tot[1].0, far_tot[1].1),
            worst.0 * 100.0,
            worst.1,
            pct(lib_tot[0].0, lib_tot[0].1),
            pct(lib_tot[1].0, lib_tot[1].1),
            pct(frr_tot[0].0, frr_tot[0].1),
            pct(frr_tot[1].0, frr_tot[1].1),
            pct(count(&tune_far, true), tune_far.len()),
            pct(count(&tune_gen, false), tune_gen.len()),
            fmt_air(&airport[0].scores[ci]),
            fmt_air(&airport[1].scores[ci]),
            if pass { "yes" } else { "**no**" },
            p50,
            p99,
            WINDOWS_S[c.window] - 1.5
        ));
    }
    let header: Vec<String> = cands.iter().map(|c| cand_name(c, &models)).collect();
    let table = |title: &str, first: &str, rows: &[Vec<String>]| {
        let mut s = format!(
            "\n### {title}\n\n| {first} | {} |\n|---|{}\n",
            header.join(" | "),
            "---|".repeat(header.len())
        );
        for r in rows {
            s.push_str(&format!("| {} |\n", r.join(" | ")));
        }
        s
    };
    md.push_str(&table(
        "FAR per pair (caller ← other), bed / clean %",
        "pair",
        &pair_rows,
    ));
    md.push_str(&table("FRR per caller, bed / clean %", "caller", &frr_rows));
    md.push_str("\n### Airport fit (today's gate reproduced offline)\n\n| utterance | fitted start ms | RMSE vs log | reproduced 1.5 s scores |\n|---|---|---|---|\n");
    for a in &airport {
        md.push_str(&format!(
            "| {} | {} | {:.3} | {} |\n",
            a.label,
            a.u_ms,
            a.rmse,
            a.reproduced
                .iter()
                .map(|x| format!("{x:.3}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    print!("{md}");
    if let Some(out) = &args.out {
        fs::write(out, &md).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("false_accept_bench: {e}");
        std::process::exit(1);
    }
}
