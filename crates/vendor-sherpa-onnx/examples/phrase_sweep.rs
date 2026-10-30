//! Qualifies a roundtrip phrase against the local Sherpa recognizer: synthesizes it N times with
//! nwr's Piper settings (the output differs per run), recognizes each clip directly with the voice
//! agent's online recognizer (no WebRTC) and reports how close the transcripts are to the phrase.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example phrase_sweep -- \
//!   --stt <stt-model-dir> --tts <tts-model-dir> --n <N> [--keyword <word>]... [--dump-misses <dir>] \
//!   [--babble <voices-root> [--seed <n>]] "<phrase>"
//! ```
//!
//! Each iteration feeds 300 ms of silence, the clip, then 2500 ms of silence (so the recognizer
//! flushes its last words), in 20 ms chunks on a fresh stream. The score is the roundtrip oracle's
//! `wordSimilarity` (`examples/voice-agent-local-sherpa/src/roundtrip-counting.ts`). `--keyword` may
//! be given several times; a synthesis counts as `keywordMissing` when any of them is not a whole
//! token of its transcript, and the table's `keyword` is the list joined with `,`.
//!
//! Text longer than one sentence (or a first sentence over 12 words) is synthesized as the same
//! engine calls nwr makes (`engine_pieces`), concatenated as the agent plays them.
//!
//! `--dump-misses <dir>` writes every synthesis that is not a perfect match (similarity < 1 or a
//! keyword missing) as `<dir>/miss-<iteration>.wav` (16 kHz mono, the clip without padding), so a
//! rare misrecognition can be compared with audio captured in a roundtrip.
//!
//! `--babble <voices-root>` qualifies the phrase under the noisy-bystander scene's conditions
//! instead (`speaker_fixtures` `s3_test_m15`, the scene `start:roundtrip-noisy-bystander` plays):
//! per caller level ([`BABBLE_CALLER_LEVELS`], -20 and -26 dBFS voiced RMS) `--n` rolls, each a
//! fresh synthesis over background babble [`BABBLE_REL_DB`] (-15) dB below the caller. The babble
//! is built as the scene builds it: two of the scene's test-set voices (Piper bundles under
//! `<voices-root>`) reading `support/background_sentences.txt` with 150-400 ms gaps, already
//! playing when the window starts (random voices, sentences and phase per roll), no babble
//! sentence starting within 1000 ms before the caller (`--babble-clear-ms 1000
//! --babble-clear-pull`). The recognizer gets what the live agent feeds it: the STT pre-roll
//! ([`BABBLE_LEAD_MS`] of babble), the caller over babble, then babble for the VAD's
//! `min_silence_duration_ms` before the utterance ends. The clean sweep (300 ms digital silence
//! before the clip) never sees a weak onset under babble: "excuse me please" was clean in 3000
//! clean runs and heard as "X cuse me, please." in the noisy-bystander scene. Misses are scored
//! with the scene's oracle ([`BABBLE_MIN_SIMILARITY`]); `--dump-misses` writes the whole input.
//!
//! Output: one JSON object on stdout; progress goes to stderr.

use std::collections::HashMap;
use std::path::PathBuf;

#[path = "support/mod.rs"]
mod support;

// `audio.rs` is private to the crate; included by path as `benches/resample.rs` does, so the sweep
// upsamples a TTS clip with the same resampler nwr's TTS output uses (no product-code change).
#[path = "../src/audio.rs"]
#[allow(dead_code, unused_imports)]
mod audio;

use support::{
    add_at, babble_clear_start, create_recognizer, create_tts, normalize_to, recognize,
    resample_linear, synthesize, synthesize_sid, write_wav, Rng, TEST_VOICES,
};

const LEAD_SILENCE_MS: usize = 300;
const TAIL_SILENCE_MS: usize = 2500;
const SAMPLE_RATE: i32 = 16_000;
/// `--babble`: caller levels (dBFS, voiced RMS) of the two noisy-bystander scenes,
/// `s3_test_m15` (speaker_fixtures `MAIN_REF_DB`) and `s3_test_m15_l26` (`--main-levels -26` in
/// `scripts/ci/ensure-speaker-fixtures.sh`).
const BABBLE_CALLER_LEVELS: [i32; 2] = [-20, -26];
/// `--babble`: babble level re the caller, the `m15` of `s3_test_m15`.
const BABBLE_REL_DB: i32 = -15;
/// `--babble`: no babble sentence starts within this many ms before the caller
/// (`--babble-clear-ms 1000 --babble-clear-pull` in `scripts/ci/ensure-speaker-fixtures.sh`).
// Stays 1000 although the CI scene now clears 2000 ms: a phrase qualified at the closer, harder clear stays valid.
const BABBLE_CLEAR_MS: usize = 1000;
/// `--babble`: the noisy-bystander roundtrip judges a caller turn with
/// `DEFAULT_BARGE_PHRASE_MIN_SIMILARITY` (`roundtrip-noisy-bystander-helpers.ts`); a roll below it
/// is a miss.
const BABBLE_MIN_SIMILARITY: f64 = 0.75;
/// `--babble`: babble before the caller's onset in the recognizer input: the STT pre-roll
/// capacity (`stt_pre_roll_capacity_ms`, `speech_pad_ms + min_speech_duration_ms`) of the VAD the
/// noisy-bystander listener runs, `VOICE_AGENT_VAD_PRESET` (speechPad 500 + minSpeech 250). Not
/// `VadConfig::default()` (550 ms).
const BABBLE_LEAD_MS: usize = 750;
/// Background sentences the scene's babble reads (`speaker_fixtures` `BACKGROUND`).
const BACKGROUND: &str = include_str!("support/background_sentences.txt");

/// Bring a mono TTS clip at `rate` to the recognizer's 16 kHz the way the product does: nwr TTS
/// output resampler to 48 kHz stereo s16le (`audio::f32_mono_to_stereo_48k_s16le_raw`), then the
/// server STT input conversion (`stereo_48k_to_mono_16k`, `mono_s16le_bytes_to_f32`). A 16 kHz
/// clip is returned unchanged. The bool is `true` when the clip was resampled.
fn to_recognizer_rate(clip: Vec<f32>, rate: i32) -> Result<(Vec<f32>, bool), String> {
    if rate == SAMPLE_RATE {
        return Ok((clip, false));
    }
    if rate <= 0 {
        return Err(format!("TTS sample rate is {rate} Hz"));
    }
    let stereo_48k = audio::f32_mono_to_stereo_48k_s16le_raw(&clip, rate as u32);
    let mono_16k = node_webrtc_rust_speech::pcm::stereo_48k_to_mono_16k(&stereo_48k);
    let bytes: Vec<u8> = mono_16k.iter().flat_map(|s| s.to_le_bytes()).collect();
    Ok((
        node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32(&bytes),
        true,
    ))
}

/// `normalizeForCompare`: lowercase, every char outside `[a-z0-9\s]` becomes a space, whitespace
/// runs collapse to one space, trimmed.
fn normalize_for_compare(text: &str) -> String {
    let lowered = text.to_lowercase();
    let replaced: String = lowered
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || is_js_whitespace(c) {
                c
            } else {
                ' '
            }
        })
        .collect();
    replaced
        .split(is_js_whitespace)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// JS `\s` (also the whitespace `trim` removes).
fn is_js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `wordSimilarity`: share of the input's words found as whole tokens in the recognized text.
fn word_similarity(input: &str, recognized: &str) -> f64 {
    let normalized_input = normalize_for_compare(input);
    let words: Vec<&str> = normalized_input
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return if recognized.trim().is_empty() {
            0.0
        } else {
            1.0
        };
    }
    let haystack = format!(" {} ", normalize_for_compare(recognized));
    let hits = words
        .iter()
        .filter(|word| haystack.contains(&format!(" {word} ")))
        .count();
    hits as f64 / words.len() as f64
}

fn all_keywords_are_tokens(text: &str, keywords: &[String]) -> bool {
    keywords
        .iter()
        .all(|keyword| keyword_is_token(text, keyword))
}

fn keyword_is_token(text: &str, keyword: &str) -> bool {
    let haystack = format!(" {} ", normalize_for_compare(text));
    haystack.contains(&format!(" {} ", normalize_for_compare(keyword)))
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Args {
    stt: PathBuf,
    tts: PathBuf,
    n: usize,
    keywords: Vec<String>,
    dump_misses: Option<PathBuf>,
    /// `--babble <voices-root>`: sweep under the noisy-bystander scene's babble.
    babble: Option<PathBuf>,
    /// `--seed` (default 20261030): babble draws (`--babble` only).
    seed: u64,
    phrase: String,
}

fn parse_args() -> Result<Args, String> {
    let mut stt = None;
    let mut tts = None;
    let mut n = None;
    let mut keywords = Vec::new();
    let mut dump_misses = None;
    let mut babble = None;
    let mut seed = 20_261_030u64;
    let mut phrase = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--stt" => stt = Some(PathBuf::from(value("--stt")?)),
            "--tts" => tts = Some(PathBuf::from(value("--tts")?)),
            "--n" => {
                n = Some(
                    value("--n")?
                        .parse::<usize>()
                        .map_err(|e| format!("--n: {e}"))?,
                )
            }
            "--keyword" => keywords.push(value("--keyword")?),
            "--dump-misses" => dump_misses = Some(PathBuf::from(value("--dump-misses")?)),
            "--babble" => babble = Some(PathBuf::from(value("--babble")?)),
            "--seed" => {
                seed = value("--seed")?
                    .parse::<u64>()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
            other => {
                if phrase.replace(other.to_string()).is_some() {
                    return Err("more than one phrase".into());
                }
            }
        }
    }
    Ok(Args {
        stt: stt.ok_or("missing --stt")?,
        tts: tts.ok_or("missing --tts")?,
        n: n.filter(|n| *n > 0).ok_or("missing or zero --n")?,
        keywords,
        dump_misses,
        babble,
        seed,
        phrase: phrase.ok_or("missing phrase")?,
    })
}

/// Score counts of one sweep (one caller level in `--babble` mode).
struct Tally {
    n: usize,
    min_observed: f64,
    below_075: usize,
    below_060: usize,
    keyword_missing: usize,
    texts: HashMap<String, usize>,
}

impl Tally {
    fn new() -> Self {
        Self {
            n: 0,
            min_observed: f64::MAX,
            below_075: 0,
            below_060: 0,
            keyword_missing: 0,
            texts: HashMap::new(),
        }
    }

    /// Scores one transcript; `true` when it is a perfect match (similarity 1, keywords present).
    fn add(&mut self, phrase: &str, keywords: &[String], text: String) -> bool {
        let similarity = word_similarity(phrase, &text);
        self.n += 1;
        self.min_observed = self.min_observed.min(similarity);
        if similarity < 0.75 {
            self.below_075 += 1;
        }
        if similarity < 0.60 {
            self.below_060 += 1;
        }
        let keywords_ok = all_keywords_are_tokens(&text, keywords);
        if !keywords_ok {
            self.keyword_missing += 1;
        }
        *self.texts.entry(text).or_insert(0) += 1;
        similarity >= 1.0 && keywords_ok
    }

    fn top_json(&self) -> String {
        let mut top: Vec<(&String, &usize)> = self.texts.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        top.truncate(5);
        top.iter()
            .map(|(text, count)| format!("[{},{count}]", json_string(text)))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn keyword_json(&self, keywords: &[String]) -> (String, String) {
        if keywords.is_empty() {
            ("null".to_string(), "null".to_string())
        } else {
            (
                json_string(&keywords.join(",")),
                self.keyword_missing.to_string(),
            )
        }
    }
}

fn ms(n: usize) -> usize {
    n * SAMPLE_RATE as usize / 1000
}

/// Every background sentence in every test-set voice, 16 kHz, each at -20 dBFS voiced RMS (the
/// scene levels each babble voice to -20 before summing). Synthesized once; the rolls draw from it.
fn babble_pool(voices_root: &std::path::Path) -> Result<Vec<Vec<Vec<f32>>>, String> {
    let sentences: Vec<&str> = BACKGROUND
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut engines: HashMap<&str, sherpa_onnx::OfflineTts> = HashMap::new();
    let mut pool = Vec::new();
    for voice in TEST_VOICES {
        if !engines.contains_key(voice.bundle) {
            engines.insert(voice.bundle, create_tts(&voices_root.join(voice.bundle))?);
        }
        let mut clips = Vec::new();
        for text in &sentences {
            let (samples, rate) = synthesize_sid(&engines[voice.bundle], text, voice.sid)?;
            clips.push(normalize_to(
                &resample_linear(&samples, rate, SAMPLE_RATE),
                -20.0,
            ));
        }
        eprintln!(
            "phrase_sweep: babble voice {} sid {} ready",
            voice.name, voice.sid
        );
        pool.push(clips);
    }
    Ok(pool)
}

/// One babble window of `len` samples with the caller turn at `onset..onset + caller_len`: two
/// distinct voices from `pool`, each reading random sentences with 150-400 ms gaps from a random
/// point before the window (already playing), sentences starting within [`BABBLE_CLEAR_MS`]
/// before the onset pulled back as the scene does. Level: unscaled (each voice at -20 dBFS).
fn babble_window(
    rng: &mut Rng,
    pool: &[Vec<Vec<f32>>],
    len: usize,
    onset: usize,
    caller_len: usize,
) -> Vec<f32> {
    let pre = rng.range(ms(2000), ms(5000));
    let total = pre + len;
    let turn = [(pre + onset, pre + onset + caller_len)];
    let first = rng.below(pool.len());
    let mut second = rng.below(pool.len() - 1);
    if second >= first {
        second += 1;
    }
    let mut mix = vec![0f32; total];
    for clips in [&pool[first], &pool[second]] {
        let mut pos = rng.range(0, ms(300));
        while pos < total {
            let clip = &clips[rng.below(clips.len())];
            let next = pos + clip.len() + ms(rng.range(150, 400));
            let start = babble_clear_start(pos, &turn, ms(BABBLE_CLEAR_MS), true);
            let used = clip.len().min(total - start);
            add_at(&mut mix, &clip[..used], start);
            pos = next;
        }
    }
    mix.split_off(pre)
}

/// `--babble`: the sweep under the noisy-bystander scene's conditions (see the module docs).
fn run_babble(
    args: &Args,
    voices_root: &std::path::Path,
    recognizer: &sherpa_onnx::OnlineRecognizer,
    tts: &sherpa_onnx::OfflineTts,
) -> Result<String, String> {
    let vad = node_webrtc_rust_speech::config::VadConfig::default();
    let lead_ms = BABBLE_LEAD_MS;
    let tail_ms = vad.min_silence_duration_ms as usize;
    let pool = babble_pool(voices_root)?;
    let mut rng = Rng::new(args.seed);
    let mut levels_json = Vec::new();
    for caller_db in BABBLE_CALLER_LEVELS {
        let mut tally = Tally::new();
        let mut misses = 0usize;
        for i in 0..args.n {
            let (raw, rate) = synthesize(tts, &args.phrase)?;
            let (clip, _) = to_recognizer_rate(raw, rate)?;
            let clip = normalize_to(&clip, caller_db as f32);
            let onset = ms(lead_ms);
            let len = onset + clip.len() + ms(tail_ms);
            let babble = babble_window(&mut rng, &pool, len, onset, clip.len());
            let mut input = normalize_to(&babble, (caller_db + BABBLE_REL_DB) as f32);
            add_at(&mut input, &clip, onset);
            let text = recognize(recognizer, &input);
            let similarity = word_similarity(&args.phrase, &text);
            let keywords_ok = all_keywords_are_tokens(&text, &args.keywords);
            if similarity < BABBLE_MIN_SIMILARITY || !keywords_ok {
                misses += 1;
            }
            if !tally.add(&args.phrase, &args.keywords, text.clone()) {
                if let Some(dir) = &args.dump_misses {
                    write_wav(
                        &dir.join(format!("miss-l{}-{i:05}.wav", caller_db.abs())),
                        &input,
                    )?;
                    eprintln!("phrase_sweep: {caller_db} dBFS miss {i}: {text:?}");
                }
            }
            if (i + 1) % 50 == 0 {
                eprintln!(
                    "phrase_sweep: {caller_db} dBFS {}/{} misses {misses}",
                    i + 1,
                    args.n
                );
            }
        }
        let (keyword_json, missing_json) = tally.keyword_json(&args.keywords);
        levels_json.push(format!(
            "{{\"callerDb\":{caller_db},\"n\":{},\"misses\":{misses},\"minObserved\":{},\"below075\":{},\"below060\":{},\"keyword\":{keyword_json},\"keywordMissing\":{missing_json},\"topTexts\":[{}]}}",
            tally.n,
            tally.min_observed,
            tally.below_075,
            tally.below_060,
            tally.top_json(),
        ));
    }
    Ok(format!(
        "{{\"phrase\":{},\"babble\":{{\"relDb\":{BABBLE_REL_DB},\"threshold\":{BABBLE_MIN_SIMILARITY},\"leadMs\":{lead_ms},\"tailMs\":{tail_ms},\"clearMs\":{BABBLE_CLEAR_MS},\"voices\":\"test\",\"seed\":{},\"levels\":[{}]}}}}",
        json_string(&args.phrase),
        args.seed,
        levels_json.join(","),
    ))
}

fn run(args: &Args) -> Result<String, String> {
    let recognizer = create_recognizer(&args.stt)?;
    let tts = create_tts(&args.tts)?;
    if let Some(dir) = &args.dump_misses {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    if let Some(voices_root) = &args.babble {
        return run_babble(args, voices_root, &recognizer, &tts);
    }
    let lead = vec![0.0_f32; SAMPLE_RATE as usize * LEAD_SILENCE_MS / 1000];
    let tail = vec![0.0_f32; SAMPLE_RATE as usize * TAIL_SILENCE_MS / 1000];

    let mut tally = Tally::new();
    let mut tts_rate: Option<i32> = None;
    let mut resampled = false;

    for i in 0..args.n {
        let (raw, rate) = synthesize(&tts, &args.phrase)?;
        match tts_rate {
            Some(first) if first != rate => {
                return Err(format!(
                    "TTS sample rate changed from {first} Hz to {rate} Hz"
                ));
            }
            _ => tts_rate = Some(rate),
        }
        let (clip, was_resampled) = to_recognizer_rate(raw, rate)?;
        resampled = was_resampled;
        let mut input = Vec::with_capacity(lead.len() + clip.len() + tail.len());
        input.extend_from_slice(&lead);
        input.extend_from_slice(&clip);
        input.extend_from_slice(&tail);
        let text = recognize(&recognizer, &input);
        if !tally.add(&args.phrase, &args.keywords, text.clone()) {
            if let Some(dir) = &args.dump_misses {
                write_wav(&dir.join(format!("miss-{i:05}.wav")), &clip)?;
                eprintln!("phrase_sweep: miss {i}: {text:?}");
            }
        }
        if (i + 1) % 50 == 0 {
            eprintln!("phrase_sweep: {}/{}", i + 1, args.n);
        }
    }

    let (keyword_json, missing_json) = tally.keyword_json(&args.keywords);
    Ok(format!(
        "{{\"phrase\":{},\"n\":{},\"minObserved\":{},\"below075\":{},\"below060\":{},\"keyword\":{keyword_json},\"keywordMissing\":{missing_json},\"ttsSampleRate\":{},\"resampled\":{resampled},\"topTexts\":[{}]}}",
        json_string(&args.phrase),
        args.n,
        tally.min_observed,
        tally.below_075,
        tally.below_060,
        tts_rate.unwrap_or(SAMPLE_RATE),
        tally.top_json(),
    ))
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "phrase_sweep: {e}\nusage: phrase_sweep --stt <dir> --tts <dir> --n <N> [--keyword <word>]... [--dump-misses <dir>] [--babble <voices-root> [--seed <n>]] \"<phrase>\""
            );
            std::process::exit(2);
        }
    };
    match run(&args) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("phrase_sweep: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dominant frequency of `samples` at `rate` by zero crossings.
    fn zero_crossing_hz(samples: &[f32], rate: i32) -> f64 {
        let crossings = samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        crossings as f64 / 2.0 / (samples.len() as f64 / f64::from(rate))
    }

    #[test]
    fn clip_at_22050_hz_is_resampled_to_16_khz() {
        let src_rate = 22_050;
        let len = src_rate as usize; // 1 s
        let sine: Vec<f32> = (0..len)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / src_rate as f32).sin())
            .collect();
        let (out, resampled) = to_recognizer_rate(sine, src_rate).expect("resample");
        assert!(resampled);
        let expected = len * SAMPLE_RATE as usize / src_rate as usize;
        assert!(
            out.len().abs_diff(expected) <= 1,
            "len {} expected {expected}",
            out.len()
        );
        let hz = zero_crossing_hz(&out, SAMPLE_RATE);
        assert!((hz - 440.0).abs() < 5.0, "frequency {hz}");
    }

    #[test]
    fn clip_at_16_khz_is_unchanged() {
        let clip: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.01).sin()).collect();
        let (out, resampled) = to_recognizer_rate(clip.clone(), SAMPLE_RATE).expect("passthrough");
        assert!(!resampled);
        assert_eq!(
            out.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
            clip.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
        );
    }

    /// `babble_window`: two distinct voices, and no babble sentence starts in the clear window
    /// before the caller's onset (the scene's `--babble-clear-ms 1000 --babble-clear-pull`).
    /// Voice k's clip is the constant 16^k, so each voice owns a 4-bit field of the summed sample
    /// (a pulled sentence may overlap the same voice's previous one; 4 bits hold that count).
    #[test]
    fn babble_window_two_voices_and_no_onset_in_clear_window() {
        let pool: Vec<Vec<Vec<f32>>> = (0..5).map(|k| vec![vec![16f32.powi(k); ms(500)]]).collect();
        let voices_at = |x: f32| -> Vec<bool> {
            let bits = x as u32;
            (0..5).map(|k| (bits >> (4 * k)) & 0xF != 0).collect()
        };
        for seed in 1..=50u64 {
            let window = babble_window(&mut Rng::new(seed), &pool, ms(3000), ms(750), ms(1000));
            assert_eq!(window.len(), ms(3000), "seed {seed}");
            let mut union = [false; 5];
            for x in &window {
                for (k, on) in voices_at(*x).into_iter().enumerate() {
                    union[k] |= on;
                }
            }
            assert_eq!(union.iter().filter(|v| **v).count(), 2, "seed {seed}");
            for i in 1..ms(750) {
                let (before, now) = (voices_at(window[i - 1]), voices_at(window[i]));
                for k in 0..5 {
                    assert!(
                        !(now[k] && !before[k]),
                        "seed {seed}: voice {k} starts at {} ms, inside the clear window",
                        i * 1000 / SAMPLE_RATE as usize
                    );
                }
            }
        }
    }

    #[test]
    fn identical_text_scores_one() {
        assert_eq!(
            word_similarity("good morning there", "good morning there"),
            1.0
        );
    }

    #[test]
    fn misheard_words_lower_the_score() {
        let sim = word_similarity(
            "hello testing recovery one two three",
            "the low testing recovery one to three",
        );
        assert!((sim - 4.0 / 6.0).abs() < 1e-12, "{sim}");
    }

    #[test]
    fn punctuation_and_case_are_ignored() {
        assert_eq!(
            word_similarity("Stop, now. Please!", "stop now please"),
            1.0
        );
    }

    #[test]
    fn keyword_must_be_a_whole_token() {
        assert!(keyword_is_token("Alpha, one two", "alpha"));
        assert!(!keyword_is_token("alphabet one two", "alpha"));
    }

    #[test]
    fn synthesis_makes_the_engine_calls_nwr_makes() {
        // Multi-sentence phrases are several engine calls in nwr; the sweep must make the same
        // ones through the product's own split, not a copy of it.
        let src = include_str!("support/mod.rs");
        let body = src
            .split("pub fn synthesize(")
            .nth(1)
            .expect("synthesize body");
        assert!(body.contains("engine_pieces(&normalize_phrase_text(text), SentenceCase::Auto)"));
        assert!(!body.contains("prepare_engine_text"));
    }

    #[test]
    fn every_keyword_must_be_present() {
        let keywords = vec!["why".to_string(), "time".to_string()];
        assert!(all_keywords_are_tokens("Why so much time?", &keywords));
        assert!(!all_keywords_are_tokens("Why so much?", &keywords));
        assert!(all_keywords_are_tokens("anything", &[]));
    }
}
