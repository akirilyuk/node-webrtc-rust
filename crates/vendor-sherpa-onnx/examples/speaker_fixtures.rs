//! Generates the speaker-ID test scenes: a **main speaker** says the qualified test phrases while
//! **other voices** talk in the background, quieter than the main voice, at several levels.
//! `speaker_bench` then measures how much of the main speech is recognized and attributed to the
//! main voice, and how much speech is attributed to the other voices.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
//!   --models <speaker dir> --main-tts <amy-low dir> --phrases <stt-phrase-qualification.json> \
//!   --out <dir> [--quick] [--voices-root <dir>] [--cache <dir>] [--main-levels <dBFS,...>]
//!   [--min-turn-gap-ms <ms>] [--bystander-db <dB>] [--long-bystander-db <dB>] [--only <scene-id-prefix>]
//! ```
//!
//! - Main voice: `vits-piper-en_US-amy-low` (`--main-tts`), sid 0. Main text = the phrases of
//!   `--phrases` with `below075 == 0` and `keywordMissing` 0 or null (a clean entry in the checked-in
//!   3000-run sweep), so recognized text is a valid oracle.
//! - Other voices (Piper bundles under `--voices-root`, default: the parent of `--models`), split
//!   so a threshold is never tuned on the voices it is tested on:
//!   - tune set: `en_US-ryan-medium`, `en_US-kristin-medium`, `en_US-hfc_male-medium`,
//!     `en_US-libritts_r-medium` sids 10 and 50;
//!   - test set: `en_US-joe-medium`, `en_US-hfc_female-medium`, `en_US-lessac-medium`,
//!     `en_US-libritts_r-medium` sids 120 and 300.
//! - Long sentences: `examples/support/long_sentences.txt` (16 customer-service style sentences of
//!   12-20 words, also spoken by the main voice). They are **not qualified** by the 3000-run
//!   sweep, so the bench measures them (WER) but never judges them pass/fail.
//! - Background text: `examples/support/background_sentences.txt` (40 short neutral sentences,
//!   none sharing a content word (4+ letters) with a main phrase or a long sentence).
//! - Levels: the main voice is normalized to [`MAIN_REF_DB`] (mean RMS over voiced 20 ms frames,
//!   frame RMS above -45 dBFS). The background voices are scaled to a level relative to that:
//!   -6, -10, -15, -20 dB, plus 0 dB for a bystander as loud as the caller.
//!
//! Scenes (files in `<out>/scenes/<id>.wav`, 48 kHz **stereo** s16le with the same audio in both
//! channels, what the agent receives, and `<id>.16k.wav`, 16 kHz mono, for embedding/STT):
//! - `main_over_babble` (S1): per main phrase (qualified phrases and long sentences) x level x set:
//!   300 ms lead, the phrase, 1500 ms tail; babble (2 simultaneous other voices, 150-400 ms between sentences) all the way through.
//! - `main_clean` (S1 without babble): the same phrases alone (test set; quick: both sets), for the
//!   clean row of the bench's denoiser WER table; `level_db` -99 stands in for minus infinity.
//! - `bystander_alone` (S2): per set 60 utterances (30 at 0 dB, 30 at -10 dB), one other voice saying one sentence at 0 or
//!   -10 dB re the main voice's level, babble at -20 dB underneath, no main speech.
//! - `conversation` (S3): per level x set one file: two enrollment utterances of the main voice,
//!   both long sentences (babble -20 dB), then 16 turns alternating main and bystander (one other
//!   voice at 0 dB); the 8 main turns alternate a qualified phrase and a long sentence (never an
//!   enrollment sentence); babble at the scene level throughout.
//!
//! - `--main-levels` (default `-20`, comma separated dBFS): besides the default scenes, test-set S3
//!   conversations at each extra main level and relative levels -15 and -10 (quick: one tune-set
//!   scene at -10 per extra level). Ids get a `_l26` / `_l14` suffix (the default -20 keeps its ids);
//!   texts, gaps and babble are the same as at -20, only the absolute level changes (bystander
//!   0 dB and babble relative to the main voice). The manifest scene carries `main_db` then.
//!
//! `manifest.json` lists per scene `id`, `kind`, `set`, `level_db`, `seed` and
//! `segments: [{start_ms, end_ms, role: main|bystander|babble, voice, sid, text, kind, expected}]`
//! (enrollment utterances carry `"enroll": true`). `kind` is `qualified`, `long` or `background`;
//! `expected` is `{speaker: "caller"|"other", transcript: <text, null for babble>}`: who the agent
//! should attribute the segment to and what it should hear. The seed of every random choice derives from 20261027 and
//! the scene id.
//!
//! **Determinism.** VITS sampling is random, so two syntheses of the same text differ. Every
//! synthesized clip is therefore stored in a cache (default `<voices-root>/speaker-tts-cache`,
//! override with `--cache`) keyed by bundle, sid and text; a second run with the same arguments reads
//! the same clips and writes byte-identical files. Delete the cache to re-roll the voices. The
//! manifest carries the sha256 of every wav, and the sha256 of `manifest.json` is printed last.
//!
//! `--min-turn-gap-ms` (default 0: the drawn 1400-2000 ms gaps): raises every S3 gap to at least
//! this many ms. The draws (and so texts, voices and babble) are unchanged, only the positions move.
//! `--bystander-db` (default 0): level of the S3 bystander turns re the main voice; babble keeps
//! the scene level. Only the gain changes, so the draws stay the same.
//! `--babble-clear-ms` (default 0): S3 babble sentences that would start within this many ms before
//! a main turn's start are pushed to start after that turn's end, or dropped when they then no
//! longer fit before the same voice's next sentence (each logged as `babble-clear: pushed|pulled|dropped`).
//! The draws stay the same, only placements move, so VAD opens on the caller, not on babble.
//! `--babble-clear-pull` (with `--babble-clear-ms`): such a sentence is pulled back to start just
//! before the window instead (it then talks through the window, with no onset in it), and nothing
//! is dropped. Pushing leaves a babble-free stretch of the window plus the dropped sentence around
//! the caller turn; the adaptive VAD reads its noise level as the 70th percentile of the last 3 s
//! of inactive frames, so a stretch of about 2.1 s or more lowers the noise level to silence, and
//! the babble returning after a short caller turn then holds the VAD open into the next turn
//! (measured: turns merged in 12 of 22 rolled scenes with push, see ROUNDTRIP.md).
//! `--long-bystanders N` (default 0): the first N S3 bystander turns read a sentence from
//! `examples/support/long_background_sentences.txt` (12 sentences of 17-19 words, about 4-6 s of
//! Piper speech, no content word shared with a main phrase or a long sentence) instead of the short
//! background pool. The short pool's 7-10 word sentences give the speaker gate's 1500 ms minimum
//! check only one or two checkable bystander turns per scene once the adaptive VAD has trimmed them
//! (−15 dB bystander, 5 dB above the babble); a long sentence keeps the VAD open past 1500 ms. The
//! text index reuses the turn's draw (index modulo the long pool), so the seed stream, gaps and
//! babble draws are unchanged and N = 0 writes byte-identical scenes. Babble keeps the short pool.
//! `--long-bystander-db` (default: the `--bystander-db` value): level of the long-pool bystander
//! turns (the first `--long-bystanders` turns) re the main voice; the other bystander turns keep
//! `--bystander-db`. At −15 dB over the babble the adaptive VAD can treat a long bystander as noise
//! and never open, so the speaker gate never scores it; CI plays the long turns at −6 dB so the VAD
//! opens and the gate has to reject them. Without the flag the scenes are byte-identical.
//! `--only` (repeatable): generate only scenes whose id starts with one of the prefixes; the
//! manifest lists only those. Without it every scene is generated (the bench default).
//!
//! `--quick`: S1 for 3 qualified phrases and 2 long sentences at -10 dB, S2 with 4 utterances per
//! set, one S3 at -10 dB (tune set) of 6 turns.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use sherpa_onnx::OfflineTts;

#[path = "support/mod.rs"]
mod support;

use support::{
    create_tts, json_escape, parse_json, resample_linear, rms_voiced_db, sha256_hex,
    synthesize_sid, write_wav, write_wav_stereo_48k, Json,
};

const SEED: u64 = 20_261_027;
const SR: i32 = 16_000;
/// Mean voiced-frame RMS of the main voice in every scene (dBFS). Piper speech has a crest factor
/// of about 20 dB, so at -20 dBFS the peaks stay below full scale (the peak guard rarely acts) and
/// the louder frames still cross the energy VAD default threshold (0.15 RMS = -16.5 dBFS).
const MAIN_REF_DB: f32 = -20.0;
const LEVELS: [i32; 4] = [-6, -10, -15, -20];
const BABBLE_UNDER_DB: i32 = -20;
const LEAD_MS: usize = 300;
const TAIL_MS: usize = 1500;
/// Gap between S3 turns. It must exceed `VadConfig::default().min_silence_duration_ms` (1100 ms)
/// by a margin, so the default VAD ends an utterance in every gap (guarded by a unit test).
const S3_GAP_MIN_MS: usize = 1400;
const S3_GAP_MAX_MS: usize = 2000;
const MAIN_BUNDLE: &str = "vits-piper-en_US-amy-low";
const BACKGROUND: &str = include_str!("support/background_sentences.txt");
const LONG: &str = include_str!("support/long_sentences.txt");
const LONG_BACKGROUND: &str = include_str!("support/long_background_sentences.txt");
const S2_PER_SET: usize = 60;
/// `level_db` of a `main_clean` scene: no babble at all (stands in for minus infinity).
const CLEAN_LEVEL_DB: i32 = -99;

fn lines_of(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `qualified` (swept, may be judged), `long` (measured only) or `background`.
fn segment_kind(role: &str, text: &str) -> &'static str {
    if role == "main" {
        if lines_of(LONG).iter().any(|l| l == text) {
            "long"
        } else {
            "qualified"
        }
    } else {
        "background"
    }
}

/// Who the agent should attribute a segment to: the caller for the main voice, else others.
fn expected_speaker(role: &str) -> &'static str {
    if role == "main" {
        "caller"
    } else {
        "other"
    }
}

fn ms(n: usize) -> usize {
    n * SR as usize / 1000
}

// ---------------------------------------------------------------------------------------------
// Seeded randomness (splitmix64) and hashing
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// Per-scene seed: independent of which other scenes are generated (so `--quick` scenes equal
/// their full-run counterparts). Masked to 48 bits so it is exact as a JSON number.
fn scene_seed(id: &str) -> u64 {
    Rng::new(SEED ^ fnv1a(id)).next_u64() & 0xFFFF_FFFF_FFFF
}

// ---------------------------------------------------------------------------------------------
// Phrases
// ---------------------------------------------------------------------------------------------

/// Main phrases: `below075 == 0` and `keywordMissing` 0 or null.
fn qualified_phrases(json_text: &str) -> Result<Vec<String>, String> {
    let root = parse_json(json_text)?;
    let Some(Json::Arr(entries)) = root.get("phrases") else {
        return Err("phrases json: no \"phrases\" array".into());
    };
    let mut out = Vec::new();
    for entry in entries {
        let phrase = match entry.get("phrase") {
            Some(Json::Str(s)) => s.clone(),
            _ => return Err("phrases json: entry without \"phrase\"".into()),
        };
        let below = matches!(entry.get("below075"), Some(Json::Num(n)) if *n == 0.0);
        let keyword_ok = matches!(
            entry.get("keywordMissing"),
            None | Some(Json::Null) | Some(Json::Num(0.0))
        );
        if below && keyword_ok {
            out.push(phrase);
        }
    }
    if out.is_empty() {
        return Err("phrases json: no qualified phrase".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Voices and synthesis (with the clip cache)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Voice {
    name: &'static str,
    bundle: &'static str,
    sid: i32,
}

const fn voice(name: &'static str, bundle: &'static str, sid: i32) -> Voice {
    Voice { name, bundle, sid }
}

const TUNE_VOICES: [Voice; 5] = [
    voice("en_US-ryan-medium", "vits-piper-en_US-ryan-medium", 0),
    voice("en_US-kristin-medium", "vits-piper-en_US-kristin-medium", 0),
    voice(
        "en_US-hfc_male-medium",
        "vits-piper-en_US-hfc_male-medium",
        0,
    ),
    voice(
        "en_US-libritts_r-medium",
        "vits-piper-en_US-libritts_r-medium",
        10,
    ),
    voice(
        "en_US-libritts_r-medium",
        "vits-piper-en_US-libritts_r-medium",
        50,
    ),
];
const TEST_VOICES: [Voice; 5] = [
    voice("en_US-joe-medium", "vits-piper-en_US-joe-medium", 0),
    voice(
        "en_US-hfc_female-medium",
        "vits-piper-en_US-hfc_female-medium",
        0,
    ),
    voice("en_US-lessac-medium", "vits-piper-en_US-lessac-medium", 0),
    voice(
        "en_US-libritts_r-medium",
        "vits-piper-en_US-libritts_r-medium",
        120,
    ),
    voice(
        "en_US-libritts_r-medium",
        "vits-piper-en_US-libritts_r-medium",
        300,
    ),
];
const MAIN_VOICE: Voice = voice("en_US-amy-low", MAIN_BUNDLE, 0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Set {
    Tune,
    Test,
}

impl Set {
    fn name(self) -> &'static str {
        match self {
            Set::Tune => "tune",
            Set::Test => "test",
        }
    }
    fn voices(self) -> &'static [Voice; 5] {
        match self {
            Set::Tune => &TUNE_VOICES,
            Set::Test => &TEST_VOICES,
        }
    }
}

struct Synth {
    voices_root: PathBuf,
    main_dir: PathBuf,
    cache_dir: PathBuf,
    engines: HashMap<String, OfflineTts>,
    clips: HashMap<String, Vec<f32>>,
    fresh: usize,
}

impl Synth {
    fn cache_name(voice: &Voice, text: &str) -> String {
        let key = format!("{}|{}|{}", voice.bundle, voice.sid, text);
        format!(
            "{:016x}{:016x}.f32",
            fnv1a(&key),
            fnv1a(&format!("{key}#2"))
        )
    }

    /// 16 kHz mono clip of `text` in `voice` (synthesized once, then read from the cache).
    fn clip(&mut self, voice: &Voice, text: &str) -> Result<Vec<f32>, String> {
        let name = Self::cache_name(voice, text);
        if let Some(clip) = self.clips.get(&name) {
            return Ok(clip.clone());
        }
        let path = self.cache_dir.join(&name);
        let (rate, samples) = match fs::read(&path) {
            Ok(bytes) if bytes.len() >= 4 => {
                let rate = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let samples = bytes[4..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect::<Vec<_>>();
                (rate, samples)
            }
            _ => {
                if !self.engines.contains_key(voice.bundle) {
                    let dir = if voice.bundle == MAIN_BUNDLE {
                        self.main_dir.clone()
                    } else {
                        self.voices_root.join(voice.bundle)
                    };
                    eprintln!("loading TTS {}", dir.display());
                    self.engines
                        .insert(voice.bundle.to_string(), create_tts(&dir)?);
                }
                let tts = &self.engines[voice.bundle];
                let (samples, rate) = synthesize_sid(tts, text, voice.sid)?;
                let mut bytes = Vec::with_capacity(4 + samples.len() * 4);
                bytes.extend_from_slice(&rate.to_le_bytes());
                for s in &samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let tmp = self.cache_dir.join(format!("{name}.tmp"));
                fs::write(&tmp, bytes).map_err(|e| format!("write cache: {e}"))?;
                fs::rename(&tmp, &path).map_err(|e| format!("rename cache: {e}"))?;
                self.fresh += 1;
                (rate, samples)
            }
        };
        let clip = resample_linear(&samples, rate, SR);
        self.clips.insert(name, clip.clone());
        Ok(clip)
    }
}

// ---------------------------------------------------------------------------------------------
// Mixing
// ---------------------------------------------------------------------------------------------

/// Scene id for a main level: unchanged at [`MAIN_REF_DB`], else `<base>_l<abs level>`.
/// Voiced level (dBFS) of an S3 bystander turn: `rel_db` re the main voice at `main_db`.
fn bystander_level_db(main_db: i32, rel_db: i32) -> f32 {
    (main_db + rel_db) as f32
}

/// `--only` filter: an empty list selects every scene.
fn scene_selected(only: &[String], id: &str) -> bool {
    only.is_empty() || only.iter().any(|prefix| id.starts_with(prefix.as_str()))
}

fn scene_id_with_main_level(base_id: &str, main_db: i32) -> String {
    if main_db == MAIN_REF_DB as i32 {
        base_id.to_string()
    } else {
        format!("{base_id}_l{}", main_db.abs())
    }
}

fn db_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// `clip` scaled so its voiced RMS is `target_db`.
fn normalize_to(clip: &[f32], target_db: f32) -> Vec<f32> {
    let current = rms_voiced_db(clip, SR);
    if !current.is_finite() {
        return clip.to_vec();
    }
    let gain = db_gain(target_db - current);
    clip.iter().map(|x| x * gain).collect()
}

fn add_at(dst: &mut [f32], src: &[f32], at: usize) {
    for (i, s) in src.iter().enumerate() {
        if let Some(d) = dst.get_mut(at + i) {
            *d += s;
        }
    }
}

struct Segment {
    start_ms: usize,
    end_ms: usize,
    role: &'static str,
    voice: String,
    sid: i32,
    text: String,
    enroll: bool,
}

fn to_ms(samples: usize) -> usize {
    samples * 1000 / SR as usize
}

fn segment(
    role: &'static str,
    voice: &Voice,
    text: &str,
    start: usize,
    len: usize,
    enroll: bool,
) -> Segment {
    Segment {
        start_ms: to_ms(start),
        end_ms: to_ms(start + len),
        role,
        voice: voice.name.to_string(),
        sid: voice.sid,
        text: text.to_string(),
        enroll,
    }
}

/// Start of a babble sentence drawn at `pos` with `--babble-clear-ms` (`clear` samples): a start
/// within `clear` before a turn's start (`turns`: main turn (start, end) in samples) moves to that
/// turn's end, repeated until it sits in no such window. `clear == 0` returns `pos`.
///
/// `pull`: the start moves to one sample before that window instead (repeated the same way, so it
/// only moves earlier and ends).
fn babble_clear_start(pos: usize, turns: &[(usize, usize)], clear: usize, pull: bool) -> usize {
    let mut pos = pos;
    while let Some(&(start, end)) = turns
        .iter()
        .find(|(start, _)| clear > 0 && start.saturating_sub(clear) <= pos && pos < *start)
    {
        if pull {
            let window = start.saturating_sub(clear);
            if window == 0 {
                return 0;
            }
            pos = window - 1;
        } else {
            pos = end;
        }
    }
    pos
}

/// Placements of one babble track's drawn sentences (`starts`, clip `lens`, in samples): `Some`
/// start per kept sentence (the drawn one, or a start moved by [`babble_clear_start`]), `None` for
/// a moved sentence that no longer ends before the next drawn start (or `len`). One entry per
/// draw, so the draws themselves never change.
fn place_babble(
    starts: &[usize],
    lens: &[usize],
    len: usize,
    turns: &[(usize, usize)],
    clear: usize,
    pull: bool,
) -> Vec<Option<usize>> {
    starts
        .iter()
        .zip(lens)
        .enumerate()
        .map(|(i, (&pos, &clip_len))| {
            let start = babble_clear_start(pos, turns, clear, pull);
            if start <= pos {
                // Unmoved, or pulled earlier: it still ends before the next drawn start.
                return Some(start);
            }
            let next = starts.get(i + 1).copied().unwrap_or(len).min(len);
            (start + clip_len <= next).then_some(start)
        })
        .collect()
}

/// Two simultaneous voices (picked from `pool`, never `exclude`) each reading background
/// sentences with 150-400 ms gaps for `len` samples, level-matched to each other and summed.
/// The result is at an arbitrary level: the caller scales it. `turns`/`clear`: see
/// [`place_babble`] (`&[]`/0 keep every drawn start).
#[allow(clippy::too_many_arguments)]
fn babble(
    synth: &mut Synth,
    rng: &mut Rng,
    sentences: &[String],
    pool: &[Voice; 5],
    exclude: Option<&Voice>,
    len: usize,
    turns: &[(usize, usize)],
    clear: usize,
    pull: bool,
) -> Result<(Vec<f32>, Vec<Segment>), String> {
    let candidates: Vec<Voice> = pool
        .iter()
        .filter(|v| exclude.is_none_or(|e| !(e.bundle == v.bundle && e.sid == v.sid)))
        .copied()
        .collect();
    let first = rng.below(candidates.len());
    let mut second = rng.below(candidates.len() - 1);
    if second >= first {
        second += 1;
    }
    let mut mix = vec![0f32; len];
    let mut segments = Vec::new();
    for voice in [candidates[first], candidates[second]] {
        let mut track = vec![0f32; len];
        let mut track_segments = Vec::new();
        let mut drawn: Vec<(usize, &String, Vec<f32>)> = Vec::new();
        let mut pos = rng.range(0, ms(300));
        while pos < len {
            let text = &sentences[rng.below(sentences.len())];
            let clip = synth.clip(&voice, text)?;
            let next = pos + clip.len() + ms(rng.range(150, 400));
            drawn.push((pos, text, clip));
            pos = next;
        }
        let starts: Vec<usize> = drawn.iter().map(|d| d.0).collect();
        let lens: Vec<usize> = drawn.iter().map(|d| d.2.len()).collect();
        let placements = place_babble(&starts, &lens, len, turns, clear, pull);
        for ((drawn_at, text, clip), placed) in drawn.iter().zip(placements) {
            let Some(pos) = placed else {
                eprintln!(
                    "babble-clear: dropped {} {:?} drawn at {} ms",
                    voice.name,
                    text,
                    to_ms(*drawn_at)
                );
                continue;
            };
            if pos != *drawn_at {
                eprintln!(
                    "babble-clear: {} {} {:?} from {} ms to {} ms",
                    if pos < *drawn_at { "pulled" } else { "pushed" },
                    voice.name,
                    text,
                    to_ms(*drawn_at),
                    to_ms(pos)
                );
            }
            let used = clip.len().min(len - pos);
            add_at(&mut track, &clip[..used], pos);
            track_segments.push(segment("babble", &voice, text, pos, used, false));
        }
        let leveled = normalize_to(&track, -20.0);
        add_at(&mut mix, &leveled, 0);
        segments.extend(track_segments);
    }
    segments.sort_by_key(|s| (s.start_ms, s.end_ms));
    Ok((mix, segments))
}

/// Babble scaled so its voiced level is `target_db`.
fn scale_babble(babble: &mut [f32], target_db: f32) {
    let current = rms_voiced_db(babble, SR);
    if current.is_finite() {
        let gain = db_gain(target_db - current);
        for s in babble.iter_mut() {
            *s *= gain;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Scenes
// ---------------------------------------------------------------------------------------------

struct Scene {
    id: String,
    kind: &'static str,
    set: Set,
    level_db: i32,
    /// Level of the main voice in dBFS (voiced RMS); [`MAIN_REF_DB`] unless `--main-levels`.
    main_db: i32,
    babble_db: i32,
    seed: u64,
    segments: Vec<Segment>,
    /// Deepest gain reduction of the peak limiter in dB (0 when it never acted).
    limiter_max_gr_db: f32,
    /// Voiced RMS in dBFS of the main voice after the limiter; `None` without main speech.
    effective_main_db: Option<f32>,
    sha256_16k: String,
    sha256_48k: String,
}

struct Ctx {
    synth: Synth,
    sentences: Vec<String>,
    phrases: Vec<String>,
    longs: Vec<String>,
    out: PathBuf,
    scenes: Vec<Scene>,
    /// `--min-turn-gap-ms`: floor of every S3 turn gap (0 keeps the drawn gap).
    min_turn_gap_ms: usize,
    /// `--only`: scene id prefixes to generate; empty generates every scene.
    only: Vec<String>,
    /// `--bystander-db`: S3 bystander level re the main voice (0 = same voiced level).
    bystander_db: i32,
    /// `--babble-clear-ms`: S3 babble keeps clear of this window before each main turn (0 = off).
    babble_clear_ms: usize,
    /// `--babble-clear-pull`: a start in that window moves to just before it, not to the turn end.
    babble_clear_pull: bool,
    /// `--long-bystanders`: the first this many S3 bystander turns use `long_background`.
    long_bystanders: usize,
    /// `--long-bystander-db`: level of the long-pool bystander turns re the main voice.
    long_bystander_db: i32,
    long_background: Vec<String>,
}

/// Level (dB re the main voice) of the `index`-th S3 bystander turn: `long_db` for the turns that
/// read the long pool (same condition as [`bystander_text`]), `short_db` for the rest.
fn bystander_rel_db(
    index: usize,
    long_bystanders: usize,
    long_pool_len: usize,
    short_db: i32,
    long_db: i32,
) -> i32 {
    if index < long_bystanders && long_pool_len > 0 {
        long_db
    } else {
        short_db
    }
}

/// Text of the `index`-th S3 bystander turn (0-based) for the turn's `drawn` index into the short
/// pool: the first `long_bystanders` turns read the long pool at `drawn % long.len()`, the rest the
/// short pool at `drawn`. The draw happens either way, so the seed stream does not change.
fn bystander_text(
    index: usize,
    drawn: usize,
    long_bystanders: usize,
    short: &[String],
    long: &[String],
) -> String {
    if index < long_bystanders && !long.is_empty() {
        long[drawn % long.len()].clone()
    } else {
        short[drawn].clone()
    }
}

impl Ctx {
    /// True when `--only` is unset or `id` starts with one of its prefixes.
    fn wants(&self, id: &str) -> bool {
        scene_selected(&self.only, id)
    }

    /// Gap after an S3 turn: the drawn gap (the draw always happens, so the seed stream and
    /// everything derived from it stay the same), raised to `--min-turn-gap-ms`.
    fn turn_gap_ms(&self, rng: &mut Rng) -> usize {
        rng.range(S3_GAP_MIN_MS, S3_GAP_MAX_MS)
            .max(self.min_turn_gap_ms)
    }

    /// Finishes a scene: peak guard, writes both wav files, records the scene.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        id: String,
        kind: &'static str,
        set: Set,
        level_db: i32,
        main_db: i32,
        babble_db: i32,
        seed: u64,
        segments: Vec<Segment>,
        mut mix: Vec<f32>,
        main_track: Option<Vec<f32>>,
    ) -> Result<(), String> {
        let (limiter_max_gr_db, effective_main_db) = apply_limiter(&mut mix, main_track.as_deref());
        let dir = self.out.join("scenes");
        let path16 = dir.join(format!("{id}.16k.wav"));
        let path48 = dir.join(format!("{id}.wav"));
        write_wav(&path16, &mix)?;
        write_wav_stereo_48k(&path48, &resample_linear(&mix, SR, 48_000))?;
        let sha256_16k = sha256_hex(&fs::read(&path16).map_err(|e| e.to_string())?);
        let sha256_48k = sha256_hex(&fs::read(&path48).map_err(|e| e.to_string())?);
        eprintln!("scene {id} ({:.1} s)", mix.len() as f32 / SR as f32);
        self.scenes.push(Scene {
            id,
            kind,
            set,
            level_db,
            main_db,
            babble_db,
            seed,
            segments,
            limiter_max_gr_db,
            effective_main_db,
            sha256_16k,
            sha256_48k,
        });
        Ok(())
    }

    fn main_clip(&mut self, phrase: &str) -> Result<Vec<f32>, String> {
        self.main_clip_at(phrase, MAIN_REF_DB as i32)
    }

    /// The main voice at `main_db` dBFS (voiced RMS).
    fn main_clip_at(&mut self, phrase: &str, main_db: i32) -> Result<Vec<f32>, String> {
        let clip = self.synth.clip(&MAIN_VOICE, phrase)?;
        Ok(normalize_to(&clip, main_db as f32))
    }

    /// S1: the main phrase (`tag` is `pNN` for a qualified phrase, `lNN` for a long sentence)
    /// over continuous babble at `level_db` re the main voice.
    fn main_over_babble(
        &mut self,
        set: Set,
        tag: &str,
        phrase: String,
        level_db: i32,
    ) -> Result<(), String> {
        let id = format!("s1_{}_{}_m{}", set.name(), tag, level_db.abs());
        if !self.wants(&id) {
            return Ok(());
        }
        let seed = scene_seed(&id);
        let mut rng = Rng::new(seed);
        let main = self.main_clip(&phrase)?;
        let total = ms(LEAD_MS) + main.len() + ms(TAIL_MS);
        let (mut bab, mut segments) = babble(
            &mut self.synth,
            &mut rng,
            &self.sentences,
            set.voices(),
            None,
            total,
            &[],
            0,
            false,
        )?;
        scale_babble(&mut bab, MAIN_REF_DB + level_db as f32);
        let mut mix = bab;
        add_at(&mut mix, &main, ms(LEAD_MS));
        let mut main_track = vec![0f32; mix.len()];
        add_at(&mut main_track, &main, ms(LEAD_MS));
        segments.push(segment(
            "main",
            &MAIN_VOICE,
            &phrase,
            ms(LEAD_MS),
            main.len(),
            false,
        ));
        segments.sort_by_key(|s| (s.start_ms, s.end_ms));
        self.finish(
            id,
            "main_over_babble",
            set,
            level_db,
            MAIN_REF_DB as i32,
            level_db,
            seed,
            segments,
            mix,
            Some(main_track),
        )
    }

    /// S1 without babble (`main_clean`): the same main phrase alone, for the clean row of the
    /// denoiser WER table. `level_db` is [`CLEAN_LEVEL_DB`], standing in for minus infinity.
    fn main_clean(&mut self, set: Set, tag: &str, phrase: String) -> Result<(), String> {
        let id = format!("s1c_{}_{}", set.name(), tag);
        if !self.wants(&id) {
            return Ok(());
        }
        let seed = scene_seed(&id);
        let main = self.main_clip(&phrase)?;
        let total = ms(LEAD_MS) + main.len() + ms(TAIL_MS);
        let mut mix = vec![0f32; total];
        add_at(&mut mix, &main, ms(LEAD_MS));
        let main_track = mix.clone();
        let segments = vec![segment(
            "main",
            &MAIN_VOICE,
            &phrase,
            ms(LEAD_MS),
            main.len(),
            false,
        )];
        self.finish(
            id,
            "main_clean",
            set,
            CLEAN_LEVEL_DB,
            MAIN_REF_DB as i32,
            CLEAN_LEVEL_DB,
            seed,
            segments,
            mix,
            Some(main_track),
        )
    }

    /// S2: one other voice saying one sentence, babble at -20 dB underneath, no main speech.
    fn bystander_alone(&mut self, set: Set, index: usize, level_db: i32) -> Result<(), String> {
        let id = format!("s2_{}_u{:02}_m{}", set.name(), index, level_db.abs());
        if !self.wants(&id) {
            return Ok(());
        }
        let seed = scene_seed(&id);
        let mut rng = Rng::new(seed);
        let voice = set.voices()[rng.below(5)];
        let text = self.sentences[rng.below(self.sentences.len())].clone();
        let clip = normalize_to(
            &self.synth.clip(&voice, &text)?,
            MAIN_REF_DB + level_db as f32,
        );
        let total = ms(LEAD_MS) + clip.len() + ms(TAIL_MS);
        let (mut bab, mut segments) = babble(
            &mut self.synth,
            &mut rng,
            &self.sentences,
            set.voices(),
            Some(&voice),
            total,
            &[],
            0,
            false,
        )?;
        scale_babble(&mut bab, MAIN_REF_DB + BABBLE_UNDER_DB as f32);
        let mut mix = bab;
        add_at(&mut mix, &clip, ms(LEAD_MS));
        segments.push(segment(
            "bystander",
            &voice,
            &text,
            ms(LEAD_MS),
            clip.len(),
            false,
        ));
        segments.sort_by_key(|s| (s.start_ms, s.end_ms));
        self.finish(
            id,
            "bystander_alone",
            set,
            level_db,
            MAIN_REF_DB as i32,
            BABBLE_UNDER_DB,
            seed,
            segments,
            mix,
            None,
        )
    }

    /// S3: two enrollment utterances of the main voice, then `turns` alternating main and
    /// bystander turns (one other voice at 0 dB); babble at -20 dB during enrollment and at
    /// `level_db` afterwards.
    fn conversation(&mut self, set: Set, level_db: i32, turns: usize) -> Result<(), String> {
        self.conversation_at(set, level_db, turns, MAIN_REF_DB as i32)
    }

    /// S3 with the main voice at `main_db` dBFS: the bystander follows at 0 dB re the main voice
    /// and the babble at `level_db` re the main voice, so only the absolute level differs. The
    /// id gets a `_l<abs main_db>` suffix unless `main_db` is [`MAIN_REF_DB`]; the seed (texts,
    /// gaps, babble) derives from the id without the suffix, so scenes of different main levels
    /// say the same things.
    fn conversation_at(
        &mut self,
        set: Set,
        level_db: i32,
        turns: usize,
        main_db: i32,
    ) -> Result<(), String> {
        let base_id = format!("s3_{}_m{}", set.name(), level_db.abs());
        let id = scene_id_with_main_level(&base_id, main_db);
        if !self.wants(&id) {
            return Ok(());
        }
        let seed = scene_seed(&base_id);
        let mut rng = Rng::new(seed);
        let qualified: Vec<String> = self
            .phrases
            .iter()
            .filter(|p| p.split_whitespace().count() <= 12)
            .cloned()
            .collect();
        let texts = plan_s3_texts(&mut rng, &qualified, &self.longs, turns.div_ceil(2))?;
        let bystander = set.voices()[rng.below(5)];

        // Build the voiced timeline first (positions), then the babble under it.
        struct Placed {
            clip: Vec<f32>,
            start: usize,
            segment: Segment,
        }
        let mut placed: Vec<Placed> = Vec::new();
        let mut pos = ms(LEAD_MS);
        for text in &texts.enroll {
            let clip = self.main_clip_at(text, main_db)?;
            let seg = segment("main", &MAIN_VOICE, text, pos, clip.len(), true);
            let start = pos;
            pos += clip.len() + ms(self.turn_gap_ms(&mut rng));
            placed.push(Placed {
                clip,
                start,
                segment: seg,
            });
        }
        let enroll_end = pos;
        let mut bystander_index = 0usize;
        for turn in 0..turns {
            if turn % 2 == 0 {
                let text = texts.main_turns[turn / 2].clone();
                let clip = self.main_clip_at(&text, main_db)?;
                let seg = segment("main", &MAIN_VOICE, &text, pos, clip.len(), false);
                let start = pos;
                pos += clip.len() + ms(self.turn_gap_ms(&mut rng));
                placed.push(Placed {
                    clip,
                    start,
                    segment: seg,
                });
            } else {
                let drawn = rng.below(self.sentences.len());
                let text = bystander_text(
                    bystander_index,
                    drawn,
                    self.long_bystanders,
                    &self.sentences,
                    &self.long_background,
                );
                let rel_db = bystander_rel_db(
                    bystander_index,
                    self.long_bystanders,
                    self.long_background.len(),
                    self.bystander_db,
                    self.long_bystander_db,
                );
                bystander_index += 1;
                let clip = normalize_to(
                    &self.synth.clip(&bystander, &text)?,
                    bystander_level_db(main_db, rel_db),
                );
                let seg = segment("bystander", &bystander, &text, pos, clip.len(), false);
                let start = pos;
                pos += clip.len() + ms(self.turn_gap_ms(&mut rng));
                placed.push(Placed {
                    clip,
                    start,
                    segment: seg,
                });
            }
        }
        let total = pos + ms(TAIL_MS);
        let main_turns: Vec<(usize, usize)> = placed
            .iter()
            .filter(|p| p.segment.role == "main")
            .map(|p| (p.start, p.start + p.clip.len()))
            .collect();
        let (mut bab, mut segments) = babble(
            &mut self.synth,
            &mut rng,
            &self.sentences,
            set.voices(),
            Some(&bystander),
            total,
            &main_turns,
            ms(self.babble_clear_ms),
            self.babble_clear_pull,
        )?;
        // One level for the whole babble track first, then a piecewise gain: -20 dB until the
        // enrollment ends, the scene level after it, with a 100 ms linear ramp between.
        scale_babble(&mut bab, main_db as f32);
        let enroll_gain = db_gain(BABBLE_UNDER_DB as f32);
        let scene_gain = db_gain(level_db as f32);
        let ramp = ms(100);
        for (i, s) in bab.iter_mut().enumerate() {
            let gain = if i < enroll_end {
                enroll_gain
            } else if i < enroll_end + ramp {
                let t = (i - enroll_end) as f32 / ramp as f32;
                enroll_gain + (scene_gain - enroll_gain) * t
            } else {
                scene_gain
            };
            *s *= gain;
        }
        let mut mix = bab;
        let mut main_track = vec![0f32; mix.len()];
        for p in placed {
            add_at(&mut mix, &p.clip, p.start);
            if p.segment.role == "main" {
                add_at(&mut main_track, &p.clip, p.start);
            }
            segments.push(p.segment);
        }
        segments.sort_by_key(|s| (s.start_ms, s.end_ms));
        self.finish(
            id,
            "conversation",
            set,
            level_db,
            main_db,
            level_db,
            seed,
            segments,
            mix,
            Some(main_track),
        )
    }
}

/// Peak limiter for a finished scene. Gain `g[n] = min(1, 0.98 / max|mix| over +-5 ms)`: instant
/// attack, 50 ms one-pole release. A scene whose peak stays at or below 0.98 is returned
/// untouched. Returns the deepest gain reduction in dB (0 or negative) and, when `main_track`
/// (the main voice alone, same length) is given, its voiced RMS in dBFS after the gain.
fn apply_limiter(mix: &mut [f32], main_track: Option<&[f32]>) -> (f32, Option<f32>) {
    const CEIL: f32 = 0.98;
    let half = ms(5);
    let alpha = 1.0 - (-1.0 / (0.05 * SR as f32)).exp();
    let n = mix.len();
    let mut gains = vec![1f32; n];
    let mut min_gain = 1f32;
    // Sliding maximum of |mix| over [i - half, i + half] with a monotonic deque.
    let mut deque: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut next = 0usize;
    let mut g = 1f32;
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let hi = (i + half).min(n.saturating_sub(1));
        while next <= hi && next < n {
            let v = mix[next].abs();
            while deque.back().is_some_and(|&b| mix[b].abs() <= v) {
                deque.pop_back();
            }
            deque.push_back(next);
            next += 1;
        }
        while deque.front().is_some_and(|&f| f + half < i) {
            deque.pop_front();
        }
        let peak = deque.front().map_or(0.0, |&f| mix[f].abs());
        let target = if peak > CEIL { CEIL / peak } else { 1.0 };
        if target < g {
            g = target;
        } else {
            g += (target - g) * alpha;
        }
        gains[i] = g;
        min_gain = min_gain.min(g);
    }
    let max_gr_db = if min_gain < 1.0 {
        20.0 * min_gain.log10()
    } else {
        0.0
    };
    let effective = main_track.and_then(|track| {
        let limited: Vec<f32> = track.iter().zip(&gains).map(|(s, g)| s * g).collect();
        let db = rms_voiced_db(&limited, SR);
        db.is_finite().then_some(db)
    });
    if min_gain < 1.0 {
        for (s, g) in mix.iter_mut().zip(&gains) {
            *s *= g;
        }
    }
    (max_gr_db, effective)
}

/// Texts of one S3 scene: two long enrollment sentences and the main turns, which alternate a
/// qualified phrase and a long sentence (qualified first). No turn reuses an enrollment sentence
/// (same text, same waveform) or another turn of the scene.
struct S3Texts {
    enroll: [String; 2],
    main_turns: Vec<String>,
}

fn plan_s3_texts(
    rng: &mut Rng,
    qualified: &[String],
    long: &[String],
    main_turns: usize,
) -> Result<S3Texts, String> {
    let nq = qualified.len();
    if long.len() < 2 {
        return Err("fewer than 2 long sentences for the enrollment".into());
    }
    let pool: Vec<&String> = qualified.iter().chain(long).collect();
    let first = nq + rng.below(long.len());
    let mut second = nq + rng.below(long.len() - 1);
    if second >= first {
        second += 1;
    }
    let turn_pool = turn_phrase_indices(pool.len(), first, second);
    let mut q_left: Vec<usize> = turn_pool.iter().copied().filter(|i| *i < nq).collect();
    let mut l_left: Vec<usize> = turn_pool.iter().copied().filter(|i| *i >= nq).collect();
    let mut texts = Vec::new();
    for turn in 0..main_turns {
        let left = if turn % 2 == 0 {
            &mut q_left
        } else {
            &mut l_left
        };
        if left.is_empty() {
            return Err("not enough distinct phrases for the conversation scene".into());
        }
        let pick = left.remove(rng.below(left.len()));
        texts.push(pool[pick].clone());
    }
    Ok(S3Texts {
        enroll: [pool[first].clone(), pool[second].clone()],
        main_turns: texts,
    })
}

/// Indices of the phrases a turn may use: every pool phrase except the two enrollment phrases
/// (the clip cache makes the same text the same waveform, so a reused phrase would be a trial
/// identical to the enrollment).
fn turn_phrase_indices(pool_len: usize, first: usize, second: usize) -> Vec<usize> {
    (0..pool_len)
        .filter(|i| *i != first && *i != second)
        .collect()
}

fn manifest_json(scenes: &[Scene]) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"tool\": \"speaker_fixtures\",\n");
    out.push_str(&format!("  \"seed\": {SEED},\n"));
    out.push_str(&format!("  \"main_ref_db\": {MAIN_REF_DB:.1},\n"));
    out.push_str("  \"scenes\": [\n");
    for (i, scene) in scenes.iter().enumerate() {
        out.push_str("    {\n");
        out.push_str(&format!("      \"id\": \"{}\",\n", json_escape(&scene.id)));
        out.push_str(&format!("      \"kind\": \"{}\",\n", scene.kind));
        out.push_str(&format!("      \"set\": \"{}\",\n", scene.set.name()));
        out.push_str(&format!("      \"level_db\": {},\n", scene.level_db));
        if scene.main_db != MAIN_REF_DB as i32 {
            // Only written for a non-default main level, so default manifests stay unchanged.
            out.push_str(&format!("      \"main_db\": {},\n", scene.main_db));
        }
        out.push_str(&format!("      \"babble_db\": {},\n", scene.babble_db));
        out.push_str(&format!("      \"seed\": {},\n", scene.seed));
        out.push_str(&format!(
            "      \"limiter_max_gr_db\": {:.3},\n",
            scene.limiter_max_gr_db
        ));
        match scene.effective_main_db {
            Some(v) => out.push_str(&format!("      \"effective_main_db\": {v:.2},\n")),
            None => out.push_str("      \"effective_main_db\": null,\n"),
        }
        out.push_str(&format!(
            "      \"wav48\": \"scenes/{}.wav\",\n",
            json_escape(&scene.id)
        ));
        out.push_str(&format!(
            "      \"wav16\": \"scenes/{}.16k.wav\",\n",
            json_escape(&scene.id)
        ));
        out.push_str(&format!(
            "      \"sha256_48k\": \"{}\",\n",
            scene.sha256_48k
        ));
        out.push_str(&format!(
            "      \"sha256_16k\": \"{}\",\n",
            scene.sha256_16k
        ));
        out.push_str("      \"segments\": [\n");
        for (j, seg) in scene.segments.iter().enumerate() {
            let enroll = if seg.enroll { ", \"enroll\": true" } else { "" };
            // Babble is not a turn: nothing in it is expected to be transcribed.
            let expected_transcript = if seg.role == "babble" {
                "null".to_string()
            } else {
                format!("\"{}\"", json_escape(&seg.text))
            };
            out.push_str(&format!(
                "        {{\"start_ms\": {}, \"end_ms\": {}, \"role\": \"{}\", \"voice\": \"{}\", \"sid\": {}, \"text\": \"{}\"{}, \"kind\": \"{}\", \"expected\": {{\"speaker\": \"{}\", \"transcript\": {}}}}}{}\n",
                seg.start_ms,
                seg.end_ms,
                seg.role,
                json_escape(&seg.voice),
                seg.sid,
                json_escape(&seg.text),
                enroll,
                segment_kind(seg.role, &seg.text),
                expected_speaker(seg.role),
                expected_transcript,
                if j + 1 < scene.segments.len() { "," } else { "" }
            ));
        }
        out.push_str("      ]\n");
        out.push_str(if i + 1 < scenes.len() {
            "    },\n"
        } else {
            "    }\n"
        });
    }
    out.push_str("  ]\n}\n");
    out
}

// ---------------------------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------------------------

struct Args {
    models: PathBuf,
    main_tts: PathBuf,
    phrases: PathBuf,
    out: PathBuf,
    voices_root: Option<PathBuf>,
    cache: Option<PathBuf>,
    quick: bool,
    /// Extra main-voice levels (dBFS) for test-set conversation scenes; `-20` is the default set.
    main_levels: Vec<i32>,
    min_turn_gap_ms: usize,
    only: Vec<String>,
    bystander_db: i32,
    babble_clear_ms: usize,
    babble_clear_pull: bool,
    long_bystanders: usize,
    /// `--long-bystander-db`; `None` uses `bystander_db`.
    long_bystander_db: Option<i32>,
}

/// Parses a bystander level flag value: an integer dB between -40 and 0.
fn parse_bystander_db(name: &str, text: &str) -> Result<i32, String> {
    let db: i32 = text.parse().map_err(|e| format!("{name} {text:?}: {e}"))?;
    if !(-40..=0).contains(&db) {
        return Err(format!("{name} {db}: must be between -40 and 0"));
    }
    Ok(db)
}

fn parse_main_levels(text: &str) -> Result<Vec<i32>, String> {
    let mut levels = Vec::new();
    for part in text.split(',') {
        let level: i32 = part
            .trim()
            .parse()
            .map_err(|e| format!("--main-levels {part:?}: {e}"))?;
        if !(-40..=-6).contains(&level) {
            return Err(format!("--main-levels {level}: must be between -40 and -6"));
        }
        if !levels.contains(&level) {
            levels.push(level);
        }
    }
    Ok(levels)
}

fn parse_args() -> Result<Args, String> {
    let mut models = None;
    let mut main_tts = None;
    let mut phrases = None;
    let mut out = None;
    let mut voices_root = None;
    let mut cache = None;
    let mut quick = false;
    let mut main_levels = vec![MAIN_REF_DB as i32];
    let mut min_turn_gap_ms = 0usize;
    let mut only = Vec::new();
    let mut bystander_db = 0i32;
    let mut babble_clear_ms = 0usize;
    let mut babble_clear_pull = false;
    let mut long_bystanders = 0usize;
    let mut long_bystander_db = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| -> Result<PathBuf, String> {
            it.next()
                .map(PathBuf::from)
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--models" => models = Some(value("--models")?),
            "--main-tts" => main_tts = Some(value("--main-tts")?),
            "--phrases" => phrases = Some(value("--phrases")?),
            "--out" => out = Some(value("--out")?),
            "--voices-root" => voices_root = Some(value("--voices-root")?),
            "--cache" => cache = Some(value("--cache")?),
            "--quick" => quick = true,
            "--main-levels" => {
                main_levels = parse_main_levels(&value("--main-levels")?.to_string_lossy())?
            }
            "--min-turn-gap-ms" => {
                let text = value("--min-turn-gap-ms")?.to_string_lossy().into_owned();
                min_turn_gap_ms = text
                    .parse()
                    .map_err(|e| format!("--min-turn-gap-ms {text:?}: {e}"))?;
            }
            "--bystander-db" => {
                let text = value("--bystander-db")?.to_string_lossy().into_owned();
                bystander_db = parse_bystander_db("--bystander-db", &text)?;
            }
            "--long-bystander-db" => {
                let text = value("--long-bystander-db")?.to_string_lossy().into_owned();
                long_bystander_db = Some(parse_bystander_db("--long-bystander-db", &text)?);
            }
            "--babble-clear-ms" => {
                let text = value("--babble-clear-ms")?.to_string_lossy().into_owned();
                babble_clear_ms = text
                    .parse()
                    .map_err(|e| format!("--babble-clear-ms {text:?}: {e}"))?;
            }
            "--babble-clear-pull" => babble_clear_pull = true,
            "--long-bystanders" => {
                let text = value("--long-bystanders")?.to_string_lossy().into_owned();
                long_bystanders = text
                    .parse()
                    .map_err(|e| format!("--long-bystanders {text:?}: {e}"))?;
            }
            "--only" => only.push(value("--only")?.to_string_lossy().into_owned()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        models: models.ok_or("--models is required")?,
        main_tts: main_tts.ok_or("--main-tts is required")?,
        phrases: phrases.ok_or("--phrases is required")?,
        out: out.ok_or("--out is required")?,
        voices_root,
        cache,
        quick,
        main_levels,
        min_turn_gap_ms,
        only,
        bystander_db,
        babble_clear_ms,
        babble_clear_pull,
        long_bystanders,
        long_bystander_db,
    })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let phrases_text = fs::read_to_string(&args.phrases)
        .map_err(|e| format!("read {}: {e}", args.phrases.display()))?;
    let phrases = qualified_phrases(&phrases_text)?;
    let sentences: Vec<String> = BACKGROUND
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    // Bundles of the other voices sit next to the speaker dir (`.models/speaker` -> `.models`).
    let voices_root = args.voices_root.clone().unwrap_or_else(|| {
        args.models
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
    });
    let cache_dir = args
        .cache
        .clone()
        .unwrap_or_else(|| voices_root.join("speaker-tts-cache"));
    fs::create_dir_all(&cache_dir).map_err(|e| format!("create {}: {e}", cache_dir.display()))?;
    fs::create_dir_all(args.out.join("scenes"))
        .map_err(|e| format!("create {}: {e}", args.out.display()))?;

    let mut ctx = Ctx {
        synth: Synth {
            voices_root,
            main_dir: args.main_tts.clone(),
            cache_dir,
            engines: HashMap::new(),
            clips: HashMap::new(),
            fresh: 0,
        },
        sentences,
        phrases,
        longs: lines_of(LONG),
        out: args.out.clone(),
        scenes: Vec::new(),
        min_turn_gap_ms: args.min_turn_gap_ms,
        only: args.only.clone(),
        bystander_db: args.bystander_db,
        babble_clear_ms: args.babble_clear_ms,
        babble_clear_pull: args.babble_clear_pull,
        long_bystanders: args.long_bystanders,
        long_bystander_db: args.long_bystander_db.unwrap_or(args.bystander_db),
        long_background: lines_of(LONG_BACKGROUND),
    };

    let sets = [Set::Tune, Set::Test];
    if args.quick {
        for set in sets {
            for phrase_idx in 0..3.min(ctx.phrases.len()) {
                let phrase = ctx.phrases[phrase_idx].clone();
                ctx.main_over_babble(set, &format!("p{phrase_idx:02}"), phrase, -10)?;
            }
            for long_idx in 0..2.min(ctx.longs.len()) {
                let phrase = ctx.longs[long_idx].clone();
                ctx.main_over_babble(set, &format!("l{long_idx:02}"), phrase, -10)?;
            }
            for index in 0..4 {
                ctx.bystander_alone(set, index, if index % 2 == 0 { 0 } else { -10 })?;
            }
            for phrase_idx in 0..3.min(ctx.phrases.len()) {
                let phrase = ctx.phrases[phrase_idx].clone();
                ctx.main_clean(set, &format!("p{phrase_idx:02}"), phrase)?;
            }
            for long_idx in 0..2.min(ctx.longs.len()) {
                let phrase = ctx.longs[long_idx].clone();
                ctx.main_clean(set, &format!("l{long_idx:02}"), phrase)?;
            }
        }
        ctx.conversation(Set::Tune, -10, 6)?;
        for &main_db in args
            .main_levels
            .iter()
            .filter(|l| **l != MAIN_REF_DB as i32)
        {
            ctx.conversation_at(Set::Tune, -10, 6, main_db)?;
        }
    } else {
        for set in sets {
            for level in LEVELS {
                for phrase_idx in 0..ctx.phrases.len() {
                    let phrase = ctx.phrases[phrase_idx].clone();
                    ctx.main_over_babble(set, &format!("p{phrase_idx:02}"), phrase, level)?;
                }
                for long_idx in 0..ctx.longs.len() {
                    let phrase = ctx.longs[long_idx].clone();
                    ctx.main_over_babble(set, &format!("l{long_idx:02}"), phrase, level)?;
                }
            }
            if set == Set::Test {
                // Babble-free S1, the clean row of the denoiser WER table.
                for phrase_idx in 0..ctx.phrases.len() {
                    let phrase = ctx.phrases[phrase_idx].clone();
                    ctx.main_clean(set, &format!("p{phrase_idx:02}"), phrase)?;
                }
                for long_idx in 0..ctx.longs.len() {
                    let phrase = ctx.longs[long_idx].clone();
                    ctx.main_clean(set, &format!("l{long_idx:02}"), phrase)?;
                }
            }
            for index in 0..S2_PER_SET {
                ctx.bystander_alone(set, index, if index % 2 == 0 { 0 } else { -10 })?;
            }
            for level in LEVELS {
                ctx.conversation(set, level, 16)?;
            }
        }
        // Other main levels: test-set conversation scenes at relative levels -15 and -10 only.
        for &main_db in args
            .main_levels
            .iter()
            .filter(|l| **l != MAIN_REF_DB as i32)
        {
            for level in [-15, -10] {
                ctx.conversation_at(Set::Test, level, 16, main_db)?;
            }
            // Tune-set twins, so the VAD sweep tunes on voices it does not report.
            for level in [-15, -10] {
                ctx.conversation_at(Set::Tune, level, 16, main_db)?;
            }
        }
    }

    let manifest = manifest_json(&ctx.scenes);
    fs::write(args.out.join("manifest.json"), &manifest)
        .map_err(|e| format!("write manifest: {e}"))?;
    eprintln!(
        "{} scenes, {} fresh syntheses (the rest came from the cache)",
        ctx.scenes.len(),
        ctx.synth.fresh
    );
    println!("manifest.json sha256 {}", sha256_hex(manifest.as_bytes()));
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("speaker_fixtures: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, amp: f32, freq: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    #[test]
    fn limiter_caps_peak_and_reports_effective_level() {
        // Peak 2.0: gain reduction 20 log10(0.98 / 2.0) = -6.2 dB, and nothing above 0.98 remains.
        let mut mix = sine(SR as usize, 2.0, 440.0);
        let (gr, eff) = apply_limiter(&mut mix, None);
        assert!(eff.is_none());
        assert!((gr + 6.2).abs() < 0.2, "max GR {gr}");
        let peak = mix.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.98 + 1e-6, "peak {peak}");

        // A -14 dBFS main voice (voiced RMS) with a short loud burst from another source: only
        // the burst frames are pulled down, the effective level stays within 1.5 dB of -14.
        let amp = 10f32.powf(-14.0 / 20.0) * 2f32.sqrt();
        let main = sine(4 * SR as usize, amp, 220.0);
        let mut mix = main.clone();
        for s in mix.iter_mut().skip(SR as usize).take(ms(5)) {
            *s += 1.5;
        }
        let (gr, eff) = apply_limiter(&mut mix, Some(&main));
        assert!(gr < -1.0, "limiter must act: {gr}");
        let eff = eff.expect("main speech present");
        assert!((eff + 14.0).abs() <= 1.5, "effective {eff}");
        let peak = mix.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.98 + 1e-6, "peak {peak}");
    }

    #[test]
    fn limiter_leaves_quiet_scenes_identical() {
        let mix = sine(2 * SR as usize, 0.5, 300.0);
        let mut out = mix.clone();
        let (gr, _) = apply_limiter(&mut out, Some(&mix));
        assert_eq!(gr, 0.0);
        assert_eq!(out, mix);
    }

    #[test]
    fn turn_phrase_indices_excludes_enrollment() {
        assert_eq!(turn_phrase_indices(7, 1, 4), vec![0, 2, 3, 5, 6]);
    }

    #[test]
    fn bystander_db_offsets_the_main_level() {
        // Default 0: the bystander keeps the main voice level (today's scenes).
        assert_eq!(bystander_level_db(-20, 0), -20.0);
        assert_eq!(bystander_level_db(-20, -15), -35.0);
        assert_eq!(bystander_level_db(-26, -15), -41.0);
    }

    #[test]
    fn only_filter_selects_by_prefix() {
        assert!(scene_selected(&[], "s1_test_p00_m10"));
        let only = vec!["s3_test_m15".to_string()];
        assert!(scene_selected(&only, "s3_test_m15"));
        assert!(scene_selected(&only, "s3_test_m15_l26"));
        assert!(!scene_selected(&only, "s3_tune_m15"));
        assert!(!scene_selected(&only, "s1_test_p00_m15"));
    }

    #[test]
    fn min_turn_gap_raises_gap_without_changing_the_draws() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..50 {
            let drawn = a.range(S3_GAP_MIN_MS, S3_GAP_MAX_MS);
            let raised = b.range(S3_GAP_MIN_MS, S3_GAP_MAX_MS).max(2500);
            assert!(drawn < 2500);
            assert_eq!(raised, 2500);
        }
        // Same number of draws: both streams stay in step.
        assert_eq!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn babble_clear_moves_starts_out_of_the_window_without_changing_the_draws() {
        let clear = ms(1000);
        // Main turns (start, end) in samples, as conversation_at builds them.
        let turns = [
            (ms(5000), ms(7000)),
            (ms(9500), ms(11000)),
            (ms(14000), ms(16000)),
        ];
        // One babble track drawn the way babble() draws it.
        let mut rng = Rng::new(11);
        let len = ms(20_000);
        let (mut starts, mut lens) = (Vec::new(), Vec::new());
        let mut pos = rng.range(0, ms(300));
        while pos < len {
            let clip = ms(rng.range(1500, 3000));
            starts.push(pos);
            lens.push(clip);
            pos += clip + ms(rng.range(150, 400));
        }
        // Off: every drawn start kept as is.
        let off = place_babble(&starts, &lens, len, &turns, 0, false);
        assert_eq!(off, starts.iter().map(|&s| Some(s)).collect::<Vec<_>>());
        // On: one entry per draw (the draws do not change), no start in [turnStart - n, turnStart).
        let on = place_babble(&starts, &lens, len, &turns, clear, false);
        assert_eq!(on.len(), starts.len());
        for (i, placed) in on.iter().enumerate() {
            let in_window = |p: usize| turns.iter().any(|&(t, _)| t - clear <= p && p < t);
            match placed {
                Some(p) => {
                    assert!(!in_window(*p), "start {p} in a cleared window");
                    if *p != starts[i] {
                        assert!(in_window(starts[i]), "only starts in a window move");
                        assert!(
                            turns.iter().any(|&(_, end)| end == *p),
                            "moved to a turn end"
                        );
                        let next = starts.get(i + 1).copied().unwrap_or(len);
                        assert!(
                            *p + lens[i] <= next,
                            "a moved sentence fits before the next"
                        );
                    }
                }
                None => assert!(in_window(starts[i]), "only moved sentences are dropped"),
            }
        }
        assert!(
            on.iter().zip(&starts).any(|(p, s)| *p != Some(*s)),
            "the fixture must exercise at least one moved or dropped start"
        );
        // Direct cases: in the window → turn end; just outside → unchanged; chained windows.
        assert_eq!(babble_clear_start(ms(4500), &turns, clear, false), ms(7000));
        assert_eq!(babble_clear_start(ms(3999), &turns, clear, false), ms(3999));
        assert_eq!(babble_clear_start(ms(5000), &turns, clear, false), ms(5000));
        let chained = [(ms(1000), ms(2000)), (ms(2500), ms(4000))];
        assert_eq!(
            babble_clear_start(ms(500), &chained, clear, false),
            ms(4000)
        );
    }

    #[test]
    fn babble_clear_pull_moves_starts_before_the_window_and_keeps_every_draw() {
        let clear = ms(1000);
        let turns = [
            (ms(5000), ms(7000)),
            (ms(9500), ms(11000)),
            (ms(14000), ms(16000)),
        ];
        let turns_before = turns;
        let mut rng = Rng::new(11);
        let len = ms(20_000);
        let (mut starts, mut lens) = (Vec::new(), Vec::new());
        let mut pos = rng.range(0, ms(300));
        while pos < len {
            let clip = ms(rng.range(1500, 3000));
            starts.push(pos);
            lens.push(clip);
            pos += clip + ms(rng.range(150, 400));
        }
        let in_window = |p: usize| turns.iter().any(|&(t, _)| t - clear <= p && p < t);
        let on = place_babble(&starts, &lens, len, &turns, clear, true);
        // Every draw is kept (nothing dropped), no start sits in a window, only window starts
        // move, and they move earlier, to one sample before a window.
        assert_eq!(on.len(), starts.len());
        let mut moved = 0;
        for (i, placed) in on.iter().enumerate() {
            let p = placed.expect("pull never drops a sentence");
            assert!(!in_window(p), "start {p} in a cleared window");
            if p != starts[i] {
                moved += 1;
                assert!(in_window(starts[i]), "only starts in a window move");
                assert!(p < starts[i], "pulled starts move earlier");
                assert!(
                    turns.iter().any(|&(t, _)| p + 1 == t - clear),
                    "to just before a window"
                );
            }
        }
        assert!(
            moved > 0,
            "the fixture must exercise at least one pulled start"
        );
        // With clear = 0 there is no window, so pull moves nothing.
        assert_eq!(
            place_babble(&starts, &lens, len, &turns, 0, true),
            starts.iter().map(|&s| Some(s)).collect::<Vec<_>>()
        );
        assert_eq!(
            turns, turns_before,
            "turn positions never depend on the babble"
        );
        // Direct cases: in the window -> just before it; outside -> unchanged; chained windows
        // keep pulling; a window that starts at 0 clamps to 0.
        assert_eq!(
            babble_clear_start(ms(4500), &turns, clear, true),
            ms(4000) - 1
        );
        assert_eq!(babble_clear_start(ms(3999), &turns, clear, true), ms(3999));
        assert_eq!(babble_clear_start(ms(5000), &turns, clear, true), ms(5000));
        let chained = [(ms(1500), ms(2000)), (ms(2400), ms(4000))];
        assert_eq!(
            babble_clear_start(ms(2000), &chained, clear, true),
            ms(500) - 1
        );
        assert_eq!(
            babble_clear_start(ms(100), &[(ms(800), ms(900))], clear, true),
            0
        );
    }

    #[test]
    fn s3_gaps_exceed_default_vad_silence() {
        let min_silence =
            node_webrtc_rust_speech::config::VadConfig::default().min_silence_duration_ms as usize;
        assert!(S3_GAP_MIN_MS >= min_silence + 300);
        assert!(S3_GAP_MAX_MS >= S3_GAP_MIN_MS);
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn scene_seed_is_stable_and_scene_specific() {
        assert_eq!(scene_seed("s1_tune_p00_m10"), scene_seed("s1_tune_p00_m10"));
        assert_ne!(scene_seed("s1_tune_p00_m10"), scene_seed("s1_tune_p01_m10"));
        assert!(scene_seed("x") < (1 << 48));
    }

    #[test]
    fn background_sentences_share_no_content_word_with_main_phrases() {
        let sentences: Vec<&str> = BACKGROUND
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(sentences.len(), 40);
        let json = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/voice-agent-local-sherpa/stt-phrase-qualification.json"),
        )
        .unwrap();
        let words = |text: &str| -> Vec<String> {
            text.to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect()
        };
        let main: Vec<String> = qualified_phrases(&json)
            .unwrap()
            .iter()
            .flat_map(|p| words(p))
            .collect();
        for sentence in &sentences {
            for word in words(sentence).into_iter().filter(|w| w.len() >= 4) {
                assert!(
                    !main.contains(&word),
                    "{sentence:?} shares {word:?} with a main phrase"
                );
            }
        }
        // Long sentences and background sentences share no 4+ letter word, either way.
        let longs = lines_of(LONG);
        assert_eq!(longs.len(), 16);
        let long_words: Vec<String> = longs
            .iter()
            .flat_map(|l| words(l))
            .filter(|w| w.len() >= 4)
            .collect();
        let bg_words: Vec<String> = sentences
            .iter()
            .flat_map(|l| words(l))
            .filter(|w| w.len() >= 4)
            .collect();
        for word in &long_words {
            assert!(
                !bg_words.contains(word),
                "a long sentence and a background sentence share {word:?}"
            );
        }
        for word in &bg_words {
            assert!(
                !long_words.contains(word),
                "a background sentence and a long sentence share {word:?}"
            );
        }
    }

    #[test]
    fn long_background_sentences_are_long_and_share_no_content_word_with_main_text() {
        let longs_bg = lines_of(LONG_BACKGROUND);
        assert_eq!(longs_bg.len(), 12);
        let json = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/voice-agent-local-sherpa/stt-phrase-qualification.json"),
        )
        .unwrap();
        let words = |text: &str| -> Vec<String> {
            text.to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect()
        };
        let mut main: Vec<String> = qualified_phrases(&json)
            .unwrap()
            .iter()
            .flat_map(|p| words(p))
            .collect();
        main.extend(lines_of(LONG).iter().flat_map(|l| words(l)));
        for sentence in &longs_bg {
            let n = words(sentence).len();
            assert!(
                (12..=20).contains(&n),
                "{sentence:?} has {n} words, want 12-20"
            );
            for word in words(sentence).into_iter().filter(|w| w.len() >= 4) {
                assert!(
                    !main.contains(&word),
                    "{sentence:?} shares {word:?} with a main phrase or a long sentence"
                );
            }
        }
    }

    #[test]
    fn bystander_text_uses_the_long_pool_for_the_first_n_turns_only() {
        let short: Vec<String> = (0..40).map(|i| format!("s{i}")).collect();
        let long: Vec<String> = (0..12).map(|i| format!("l{i}")).collect();
        // N = 0: always the drawn short sentence (byte-identical scenes).
        for index in 0..8 {
            assert_eq!(bystander_text(index, 25, 0, &short, &long), "s25");
        }
        // N = 2: turns 0 and 1 read the long pool at drawn % 12, later turns the short pool.
        assert_eq!(bystander_text(0, 25, 2, &short, &long), "l1");
        assert_eq!(bystander_text(1, 7, 2, &short, &long), "l7");
        assert_eq!(bystander_text(2, 25, 2, &short, &long), "s25");
        // An empty long pool falls back to the short one.
        assert_eq!(bystander_text(0, 3, 2, &short, &[]), "s3");
    }

    #[test]
    fn long_bystander_db_sets_the_level_of_the_long_pool_turns_only() {
        // Without the flag the long level equals --bystander-db: every turn -15 (byte-identical).
        for index in 0..8 {
            assert_eq!(bystander_rel_db(index, 2, 12, -15, -15), -15);
        }
        // --long-bystander-db -6 with --long-bystanders 2: turns 0 and 1 at -6, the rest -15.
        assert_eq!(bystander_rel_db(0, 2, 12, -15, -6), -6);
        assert_eq!(bystander_rel_db(1, 2, 12, -15, -6), -6);
        assert_eq!(bystander_rel_db(2, 2, 12, -15, -6), -15);
        assert_eq!(bystander_rel_db(7, 2, 12, -15, -6), -15);
        // No long turns, or an empty long pool (short text): the short level.
        assert_eq!(bystander_rel_db(0, 0, 12, -15, -6), -15);
        assert_eq!(bystander_rel_db(0, 2, 0, -15, -6), -15);
        // Absolute level at the CI main level -26: -32 dBFS long, -41 dBFS short.
        assert_eq!(
            bystander_level_db(-26, bystander_rel_db(0, 2, 12, -15, -6)),
            -32.0
        );
        assert_eq!(
            bystander_level_db(-26, bystander_rel_db(2, 2, 12, -15, -6)),
            -41.0
        );
    }

    #[test]
    fn bystander_db_flags_parse_and_reject_out_of_range() {
        assert_eq!(parse_bystander_db("--long-bystander-db", "-6"), Ok(-6));
        assert_eq!(parse_bystander_db("--bystander-db", "0"), Ok(0));
        assert!(parse_bystander_db("--long-bystander-db", "3").is_err());
        assert!(parse_bystander_db("--long-bystander-db", "-41").is_err());
        assert!(parse_bystander_db("--long-bystander-db", "x").is_err());
    }

    #[test]
    fn manifest_segments_have_expected() {
        let long = lines_of(LONG)[0].clone();
        let seg = |role: &'static str, text: &str, enroll: bool| Segment {
            start_ms: 0,
            end_ms: 1000,
            role,
            voice: "v".into(),
            sid: 0,
            text: text.into(),
            enroll,
        };
        let scene = Scene {
            id: "s3_test_m10".into(),
            kind: "conversation",
            set: Set::Test,
            level_db: -10,
            main_db: -20,
            babble_db: -10,
            seed: 1,
            segments: vec![
                seg("main", &long, true),
                seg("main", "charlie one two three", false),
                seg("bystander", "Rain tapped on the glass.", false),
                seg("babble", "Some background words.", false),
            ],
            limiter_max_gr_db: 0.0,
            effective_main_db: Some(-20.0),
            sha256_16k: "a".into(),
            sha256_48k: "b".into(),
        };
        let root = parse_json(&manifest_json(&[scene])).unwrap();
        let Some(Json::Arr(scenes)) = root.get("scenes") else {
            panic!("no scenes");
        };
        let Some(Json::Arr(segs)) = scenes[0].get("segments") else {
            panic!("no segments");
        };
        let field = |s: &Json, key: &str| match s.get(key) {
            Some(Json::Str(v)) => v.clone(),
            other => panic!("{key}: {other:?}"),
        };
        let expected = |s: &Json| s.get("expected").cloned().expect("expected object");
        let kinds: Vec<String> = segs.iter().map(|s| field(s, "kind")).collect();
        assert_eq!(kinds, ["long", "qualified", "background", "background"]);
        let speakers: Vec<String> = segs
            .iter()
            .map(|s| field(&expected(s), "speaker"))
            .collect();
        assert_eq!(speakers, ["caller", "caller", "other", "other"]);
        assert_eq!(
            field(&expected(&segs[1]), "transcript"),
            "charlie one two three"
        );
        // Babble is not expected to be transcribed.
        assert!(matches!(
            expected(&segs[3]).get("transcript"),
            Some(Json::Null)
        ));
    }

    #[test]
    fn s3_enrollment_uses_two_long_sentences_not_reused() {
        let longs = lines_of(LONG);
        let qualified: Vec<String> = (0..30).map(|i| format!("qualified phrase {i}")).collect();
        for seed in 0..50u64 {
            let mut rng = Rng::new(seed);
            let texts = plan_s3_texts(&mut rng, &qualified, &longs, 8).unwrap();
            assert_ne!(texts.enroll[0], texts.enroll[1]);
            for e in &texts.enroll {
                assert!(longs.contains(e), "enrollment {e:?} is not a long sentence");
                assert!(!texts.main_turns.contains(e), "enrollment {e:?} reused");
            }
            assert_eq!(texts.main_turns.len(), 8);
            for (i, t) in texts.main_turns.iter().enumerate() {
                assert_eq!(longs.contains(t), i % 2 == 1, "turn {i} kind");
            }
            let mut sorted = texts.main_turns.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), 8, "a turn text repeats");
        }
    }

    #[test]
    fn qualified_phrases_filters_on_sweep_result() {
        let json = r#"{"phrases":[
            {"phrase":"a \"b\"","below075":0,"keywordMissing":null},
            {"phrase":"c","below075":1,"keywordMissing":0},
            {"phrase":"d","below075":0,"keywordMissing":2},
            {"phrase":"e","below075":0,"keywordMissing":0}]}"#;
        assert_eq!(qualified_phrases(json).unwrap(), vec!["a \"b\"", "e"]);
    }

    #[test]
    fn main_level_ids_keep_the_default_and_suffix_the_rest() {
        assert_eq!(scene_id_with_main_level("s3_test_m10", -20), "s3_test_m10");
        assert_eq!(
            scene_id_with_main_level("s3_test_m10", -26),
            "s3_test_m10_l26"
        );
        assert_eq!(
            scene_id_with_main_level("s3_test_m15", -14),
            "s3_test_m15_l14"
        );
    }

    #[test]
    fn main_levels_parse_and_reject_out_of_range() {
        assert_eq!(
            parse_main_levels("-26,-20,-14,-20").unwrap(),
            vec![-26, -20, -14]
        );
        assert!(parse_main_levels("-50").is_err());
        assert!(parse_main_levels("loud").is_err());
    }
}
