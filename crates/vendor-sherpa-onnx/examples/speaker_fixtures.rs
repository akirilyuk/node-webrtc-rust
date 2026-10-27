//! Generates the speaker-ID test scenes: a **main speaker** says the qualified test phrases while
//! **other voices** talk in the background, quieter than the main voice, at several levels.
//! `speaker_bench` then measures how much of the main speech is recognized and attributed to the
//! main voice, and how much speech is attributed to the other voices.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example speaker_fixtures -- \
//!   --models <speaker dir> --main-tts <amy-low dir> --phrases <stt-phrase-qualification.json> \
//!   --out <dir> [--quick] [--voices-root <dir>] [--cache <dir>]
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
//! - Background text: `examples/support/background_sentences.txt` (40 short neutral sentences,
//!   none sharing a content word with a main phrase).
//! - Levels: the main voice is normalized to [`MAIN_REF_DB`] (mean RMS over voiced 20 ms frames,
//!   frame RMS above -45 dBFS). The background voices are scaled to a level relative to that:
//!   -6, -10, -15, -20 dB, plus 0 dB for a bystander as loud as the caller.
//!
//! Scenes (files in `<out>/scenes/<id>.wav`, 48 kHz **stereo** s16le with the same audio in both
//! channels, what the agent receives, and `<id>.16k.wav`, 16 kHz mono, for embedding/STT):
//! - `main_over_babble` (S1): per main phrase x level x set: 300 ms lead, the phrase, 1500 ms
//!   tail; babble (2 simultaneous other voices, 150-400 ms between sentences) all the way through.
//! - `bystander_alone` (S2): per set 20 utterances, one other voice saying one sentence at 0 or
//!   -10 dB re the main voice's level, babble at -20 dB underneath, no main speech.
//! - `conversation` (S3): per level x set one file: two enrollment utterances of the main voice
//!   (babble -20 dB), then 16 turns alternating main and bystander (one other voice at 0 dB), babble
//!   at the scene level throughout.
//!
//! `manifest.json` lists per scene `id`, `kind`, `set`, `level_db`, `seed` and
//! `segments: [{start_ms, end_ms, role: main|bystander|babble, voice, sid, text}]` (enrollment
//! utterances carry `"enroll": true`). The seed of every random choice derives from 20261027 and
//! the scene id.
//!
//! **Determinism.** VITS sampling is random, so two syntheses of the same text differ. Every
//! synthesized clip is therefore stored in a cache (default `<voices-root>/speaker-tts-cache`,
//! override with `--cache`) keyed by bundle, sid and text; a second run with the same arguments reads
//! the same clips and writes byte-identical files. Delete the cache to re-roll the voices. The
//! manifest carries the sha256 of every wav, and the sha256 of `manifest.json` is printed last.
//!
//! `--quick`: S1 for 3 phrases at -10 dB, S2 with 4 utterances per set, one S3 at -10 dB (tune
//! set) of 6 turns.

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

/// Two simultaneous voices (picked from `pool`, never `exclude`) each reading background
/// sentences with 150-400 ms gaps for `len` samples, level-matched to each other and summed.
/// The result is at an arbitrary level: the caller scales it.
fn babble(
    synth: &mut Synth,
    rng: &mut Rng,
    sentences: &[String],
    pool: &[Voice; 5],
    exclude: Option<&Voice>,
    len: usize,
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
        let mut pos = rng.range(0, ms(300));
        while pos < len {
            let text = &sentences[rng.below(sentences.len())];
            let clip = synth.clip(&voice, text)?;
            let used = clip.len().min(len - pos);
            add_at(&mut track, &clip[..used], pos);
            track_segments.push(segment("babble", &voice, text, pos, used, false));
            pos += clip.len() + ms(rng.range(150, 400));
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
    babble_db: i32,
    seed: u64,
    segments: Vec<Segment>,
    peak_scale: f32,
    sha256_16k: String,
    sha256_48k: String,
}

struct Ctx {
    synth: Synth,
    sentences: Vec<String>,
    phrases: Vec<String>,
    out: PathBuf,
    scenes: Vec<Scene>,
}

impl Ctx {
    /// Finishes a scene: peak guard, writes both wav files, records the scene.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        id: String,
        kind: &'static str,
        set: Set,
        level_db: i32,
        babble_db: i32,
        seed: u64,
        segments: Vec<Segment>,
        mut mix: Vec<f32>,
    ) -> Result<(), String> {
        let peak = mix.iter().fold(0f32, |m, s| m.max(s.abs()));
        let peak_scale = if peak > 0.98 { 0.98 / peak } else { 1.0 };
        if peak_scale != 1.0 {
            for s in mix.iter_mut() {
                *s *= peak_scale;
            }
        }
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
            babble_db,
            seed,
            segments,
            peak_scale,
            sha256_16k,
            sha256_48k,
        });
        Ok(())
    }

    fn main_clip(&mut self, phrase: &str) -> Result<Vec<f32>, String> {
        let clip = self.synth.clip(&MAIN_VOICE, phrase)?;
        Ok(normalize_to(&clip, MAIN_REF_DB))
    }

    /// S1: the main phrase over continuous babble at `level_db` re the main voice.
    fn main_over_babble(
        &mut self,
        set: Set,
        phrase_idx: usize,
        level_db: i32,
    ) -> Result<(), String> {
        let id = format!("s1_{}_p{:02}_m{}", set.name(), phrase_idx, level_db.abs());
        let seed = scene_seed(&id);
        let mut rng = Rng::new(seed);
        let phrase = self.phrases[phrase_idx].clone();
        let main = self.main_clip(&phrase)?;
        let total = ms(LEAD_MS) + main.len() + ms(TAIL_MS);
        let (mut bab, mut segments) = babble(
            &mut self.synth,
            &mut rng,
            &self.sentences,
            set.voices(),
            None,
            total,
        )?;
        scale_babble(&mut bab, MAIN_REF_DB + level_db as f32);
        let mut mix = bab;
        add_at(&mut mix, &main, ms(LEAD_MS));
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
            level_db,
            seed,
            segments,
            mix,
        )
    }

    /// S2: one other voice saying one sentence, babble at -20 dB underneath, no main speech.
    fn bystander_alone(&mut self, set: Set, index: usize, level_db: i32) -> Result<(), String> {
        let id = format!("s2_{}_u{:02}_m{}", set.name(), index, level_db.abs());
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
            BABBLE_UNDER_DB,
            seed,
            segments,
            mix,
        )
    }

    /// S3: two enrollment utterances of the main voice, then `turns` alternating main and
    /// bystander turns (one other voice at 0 dB); babble at -20 dB during enrollment and at
    /// `level_db` afterwards.
    fn conversation(&mut self, set: Set, level_db: i32, turns: usize) -> Result<(), String> {
        let id = format!("s3_{}_m{}", set.name(), level_db.abs());
        let seed = scene_seed(&id);
        let mut rng = Rng::new(seed);
        let pool: Vec<String> = self
            .phrases
            .iter()
            .filter(|p| p.split_whitespace().count() <= 12)
            .cloned()
            .collect();
        if pool.len() < 3 {
            return Err("fewer than 3 short qualified phrases for the conversation scene".into());
        }
        let bystander = set.voices()[rng.below(5)];

        // Build the voiced timeline first (positions), then the babble under it.
        struct Placed {
            clip: Vec<f32>,
            start: usize,
            segment: Segment,
        }
        let mut placed: Vec<Placed> = Vec::new();
        let mut pos = ms(LEAD_MS);
        let first = rng.below(pool.len());
        let mut second = rng.below(pool.len() - 1);
        if second >= first {
            second += 1;
        }
        let turn_pool = turn_phrase_indices(pool.len(), first, second);
        for phrase_idx in [first, second] {
            let text = pool[phrase_idx].clone();
            let clip = self.main_clip(&text)?;
            let seg = segment("main", &MAIN_VOICE, &text, pos, clip.len(), true);
            let start = pos;
            pos += clip.len() + ms(rng.range(S3_GAP_MIN_MS, S3_GAP_MAX_MS));
            placed.push(Placed {
                clip,
                start,
                segment: seg,
            });
        }
        let enroll_end = pos;
        for turn in 0..turns {
            if turn % 2 == 0 {
                let text = pool[turn_pool[rng.below(turn_pool.len())]].clone();
                let clip = self.main_clip(&text)?;
                let seg = segment("main", &MAIN_VOICE, &text, pos, clip.len(), false);
                let start = pos;
                pos += clip.len() + ms(rng.range(S3_GAP_MIN_MS, S3_GAP_MAX_MS));
                placed.push(Placed {
                    clip,
                    start,
                    segment: seg,
                });
            } else {
                let text = self.sentences[rng.below(self.sentences.len())].clone();
                let clip = normalize_to(&self.synth.clip(&bystander, &text)?, MAIN_REF_DB);
                let seg = segment("bystander", &bystander, &text, pos, clip.len(), false);
                let start = pos;
                pos += clip.len() + ms(rng.range(S3_GAP_MIN_MS, S3_GAP_MAX_MS));
                placed.push(Placed {
                    clip,
                    start,
                    segment: seg,
                });
            }
        }
        let total = pos + ms(TAIL_MS);
        let (mut bab, mut segments) = babble(
            &mut self.synth,
            &mut rng,
            &self.sentences,
            set.voices(),
            Some(&bystander),
            total,
        )?;
        // One level for the whole babble track first, then a piecewise gain: -20 dB until the
        // enrollment ends, the scene level after it, with a 100 ms linear ramp between.
        scale_babble(&mut bab, MAIN_REF_DB);
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
        for p in placed {
            add_at(&mut mix, &p.clip, p.start);
            segments.push(p.segment);
        }
        segments.sort_by_key(|s| (s.start_ms, s.end_ms));
        self.finish(
            id,
            "conversation",
            set,
            level_db,
            level_db,
            seed,
            segments,
            mix,
        )
    }
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
        out.push_str(&format!("      \"babble_db\": {},\n", scene.babble_db));
        out.push_str(&format!("      \"seed\": {},\n", scene.seed));
        out.push_str(&format!("      \"peak_scale\": {:.6},\n", scene.peak_scale));
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
            out.push_str(&format!(
                "        {{\"start_ms\": {}, \"end_ms\": {}, \"role\": \"{}\", \"voice\": \"{}\", \"sid\": {}, \"text\": \"{}\"{}}}{}\n",
                seg.start_ms,
                seg.end_ms,
                seg.role,
                json_escape(&seg.voice),
                seg.sid,
                json_escape(&seg.text),
                enroll,
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
}

fn parse_args() -> Result<Args, String> {
    let mut models = None;
    let mut main_tts = None;
    let mut phrases = None;
    let mut out = None;
    let mut voices_root = None;
    let mut cache = None;
    let mut quick = false;
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
        out: args.out.clone(),
        scenes: Vec::new(),
    };

    let sets = [Set::Tune, Set::Test];
    if args.quick {
        for set in sets {
            for phrase_idx in 0..3.min(ctx.phrases.len()) {
                ctx.main_over_babble(set, phrase_idx, -10)?;
            }
            for index in 0..4 {
                ctx.bystander_alone(set, index, if index % 2 == 0 { 0 } else { -10 })?;
            }
        }
        ctx.conversation(Set::Tune, -10, 6)?;
    } else {
        for set in sets {
            for level in LEVELS {
                for phrase_idx in 0..ctx.phrases.len() {
                    ctx.main_over_babble(set, phrase_idx, level)?;
                }
            }
            for index in 0..20 {
                ctx.bystander_alone(set, index, if index % 2 == 0 { 0 } else { -10 })?;
            }
            for level in LEVELS {
                ctx.conversation(set, level, 16)?;
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

    #[test]
    fn turn_phrase_indices_excludes_enrollment() {
        assert_eq!(turn_phrase_indices(7, 1, 4), vec![0, 2, 3, 5, 6]);
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
        for sentence in sentences {
            for word in words(sentence).into_iter().filter(|w| w.len() >= 4) {
                assert!(
                    !main.contains(&word),
                    "{sentence:?} shares {word:?} with a main phrase"
                );
            }
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
}
