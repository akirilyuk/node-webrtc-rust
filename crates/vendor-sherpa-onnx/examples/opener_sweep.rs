//! Measures how a lone short opener ("Hi!") at the start of a TTS reply is rendered by every
//! English voice of the Sherpa TTS catalog, as written and merged into the next sentence:
//!
//! - as written: `"Hi! Ask me anything about VoiceThere."`
//! - merged: `"Hi, ask me anything about VoiceThere."` (terminal `!` → `,`, next word lowercased)
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example opener_sweep -- \
//!   --catalog <sherpa-tts-catalog.json> --models <dir> --stt <stt-model-dir> --n <N> \
//!   --out <json> [--only <model-id>]...
//! ```
//!
//! `--models` holds one directory per catalog `bundle`. `--only` may be given several times; without
//! it every English model is swept. Non-English models are listed as skipped (no synthesis).
//!
//! Each clip is the engine calls nwr makes ([`synthesize_at`]: `engine_pieces` under
//! `SentenceCase::Auto`, the catalog speaker id, the given speed), concatenated as played and brought
//! to 16 kHz like the product (`to_recognizer_rate`). Speeds: nwr's Sherpa default (`parse_speed`:
//! 1.0 when neither the TTS `model` field nor `SHERPA_TTS_SPEED` sets one) and 1.1.
//!
//! Metrics per clip (16 kHz mono, 10 ms frames, see [`clip_metrics`]): `headDb` over the first
//! 500 ms after onset, `bodyDb` over the rest, `deltaDb = headDb − bodyDb`, and `gapMs`, the longest
//! near-silent run in the first second. The clip is then recognized with the voice agent's online
//! recognizer (300 ms lead / 2500 ms tail silence like `phrase_sweep`) and `firstWordOk` says whether
//! the transcript's first token is the opener's first word (`ok` ≡ `okay`).
//!
//! Output: one JSON object written to `--out` (rewritten after each bundle); progress on stderr.

use std::path::{Path, PathBuf};

#[path = "support/mod.rs"]
mod support;

// `audio.rs` is private to the crate; included by path as `phrase_sweep` does, so clips are
// resampled with the same resampler nwr's TTS output uses.
#[path = "../src/audio.rs"]
#[allow(dead_code, unused_imports)]
mod audio;

use sherpa_onnx::{GenerationConfig, OfflineTts};
use support::{create_recognizer, create_tts, json_escape, parse_json, recognize, Json};

const SAMPLE_RATE: i32 = 16_000;
const LEAD_SILENCE_MS: usize = 300;
const TAIL_SILENCE_MS: usize = 2500;
/// 10 ms at 16 kHz.
const FRAME: usize = 160;
const HEAD_MS: usize = 500;
const GAP_WINDOW_MS: usize = 1000;
const ONSET_BELOW_PEAK_DB: f64 = 40.0;
const FLOOR_DB: f64 = -120.0;

/// nwr's Sherpa default speed (`parse_speed` in `src/tts.rs`) and the speed the reported voice uses.
const SPEEDS: [f32; 2] = [1.0, 1.1];

const OPENERS: [&str; 20] = [
    "Hi!", "Hey!", "Sure!", "Yes!", "No!", "Okay!", "OK!", "Great!", "Thanks!", "Right!", "Hello!",
    "Well!", "So!", "Oh!", "Wow!", "Yep!", "Nope!", "Cool!", "Done!", "Got it!",
];
const BODY: &str = "Ask me anything about VoiceThere.";

/// Same conversion as `phrase_sweep::to_recognizer_rate`: nwr TTS output resampler to 48 kHz stereo
/// s16le, then the server STT input conversion to 16 kHz mono f32.
fn to_recognizer_rate(clip: Vec<f32>, rate: i32) -> Result<Vec<f32>, String> {
    if rate == SAMPLE_RATE {
        return Ok(clip);
    }
    if rate <= 0 {
        return Err(format!("TTS sample rate is {rate} Hz"));
    }
    let stereo_48k = audio::f32_mono_to_stereo_48k_s16le_raw(&clip, rate as u32);
    let mono_16k = node_webrtc_rust_speech::pcm::stereo_48k_to_mono_16k(&stereo_48k);
    let bytes: Vec<u8> = mono_16k.iter().flat_map(|s| s.to_le_bytes()).collect();
    Ok(node_webrtc_rust_speech::pcm::mono_s16le_bytes_to_f32(
        &bytes,
    ))
}

/// `support::synthesize_sid` with an explicit speed: the engine calls nwr's Sherpa TTS makes
/// (`normalize_phrase_text`, then `engine_pieces` under `SentenceCase::Auto`), one call per piece
/// with `sid` and `speed` as `synthesize_sentence` passes them, samples concatenated as played.
fn synthesize_at(
    tts: &OfflineTts,
    text: &str,
    sid: i32,
    speed: f32,
) -> Result<(Vec<f32>, i32), String> {
    use node_webrtc_rust_speech::config::SentenceCase;
    use node_webrtc_rust_speech::tts_cache::normalize_phrase_text;
    use node_webrtc_rust_vendor_sherpa_onnx::engine_pieces;
    let config = GenerationConfig {
        sid,
        speed,
        ..Default::default()
    };
    let mut samples = Vec::new();
    for piece in engine_pieces(&normalize_phrase_text(text), SentenceCase::Auto) {
        let audio = tts
            .generate_with_config(&piece, &config, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| format!("OfflineTts generation returned no audio for {piece:?}"))?;
        samples.extend_from_slice(audio.samples());
    }
    Ok((samples, tts.sample_rate()))
}

/// `"<Opener> Ask me anything about VoiceThere."`
fn as_written(opener: &str) -> String {
    format!("{opener} {BODY}")
}

/// The opener's terminal `!` becomes `,` and the next word is lowercased:
/// `"Got it!"` → `"Got it, ask me anything about VoiceThere."`.
fn merged(opener: &str) -> String {
    let head = opener.strip_suffix('!').unwrap_or(opener);
    let mut body = BODY.chars();
    let first = body
        .next()
        .map(|c| c.to_lowercase().to_string())
        .unwrap_or_default();
    format!("{head}, {first}{}", body.as_str())
}

/// Lowercased first word of the opener, with `ok` folded to `okay`.
fn expected_first_word(opener: &str) -> String {
    let word = opener
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    fold_ok(word)
}

fn fold_ok(word: String) -> String {
    if word == "ok" {
        "okay".to_string()
    } else {
        word
    }
}

/// First transcript token, lowercased, punctuation stripped, `ok` folded to `okay`.
fn first_token(transcript: &str) -> String {
    let lowered = transcript.to_lowercase();
    let word = lowered
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect::<String>()
        })
        .find(|w| !w.is_empty())
        .unwrap_or_default();
    fold_ok(word)
}

fn rms_db(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return FLOOR_DB;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    let rms = (sum / samples.len() as f64).sqrt();
    if rms <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * rms.log10()).max(FLOOR_DB)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Metrics {
    onset_ms: usize,
    end_ms: usize,
    head_db: f64,
    /// `None` when the clip ends within 500 ms of onset.
    body_db: Option<f64>,
    delta_db: Option<f64>,
    gap_ms: usize,
}

/// Metrics of a 16 kHz mono clip in 10 ms frames. Onset is the first frame whose RMS is at least
/// peak frame RMS − 40 dB, end the last such frame. `head_db` is the RMS dBFS of
/// [onset, onset + 500 ms), `body_db` of [onset + 500 ms, end] (inclusive of the end frame),
/// `gap_ms` the longest run of frames below the threshold inside [onset, onset + 1000 ms).
fn clip_metrics(clip: &[f32]) -> Option<Metrics> {
    let frames: Vec<f64> = clip.chunks(FRAME).map(rms_db).collect();
    let peak = frames.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !peak.is_finite() || peak <= FLOOR_DB {
        return None;
    }
    let threshold = peak - ONSET_BELOW_PEAK_DB;
    let onset = frames.iter().position(|db| *db >= threshold)?;
    let end = frames.iter().rposition(|db| *db >= threshold)?;
    let head_frames = HEAD_MS / 10;
    let head_end = (onset + head_frames).min(frames.len());
    let sample_range = |from: usize, to: usize| {
        let a = (from * FRAME).min(clip.len());
        let b = (to * FRAME).min(clip.len());
        &clip[a..b]
    };
    let head_db = rms_db(sample_range(onset, head_end));
    let body_db =
        (onset + head_frames <= end).then(|| rms_db(sample_range(onset + head_frames, end + 1)));
    let gap_end = (onset + GAP_WINDOW_MS / 10).min(frames.len());
    let mut longest = 0usize;
    let mut run = 0usize;
    for db in &frames[onset..gap_end] {
        if *db < threshold {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    Some(Metrics {
        onset_ms: onset * 10,
        end_ms: end * 10 + 10,
        head_db,
        body_db,
        delta_db: body_db.map(|b| head_db - b),
        gap_ms: longest * 10,
    })
}

struct CatalogModel {
    id: String,
    bundle: String,
    language: String,
    kind: String,
    speaker_id: i32,
}

fn str_field(obj: &Json, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(Json::Str(s)) => Ok(s.clone()),
        _ => Err(format!("catalog model missing string {key}")),
    }
}

fn read_catalog(path: &Path) -> Result<Vec<CatalogModel>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let root = parse_json(&text)?;
    let Some(Json::Arr(models)) = root.get("models") else {
        return Err("catalog has no models array".into());
    };
    models
        .iter()
        .map(|m| {
            let speaker_id = match m.get("speakerId") {
                Some(Json::Num(n)) => *n as i32,
                _ => 0,
            };
            Ok(CatalogModel {
                id: str_field(m, "id")?,
                bundle: str_field(m, "bundle")?,
                language: str_field(m, "language")?,
                kind: str_field(m, "kind")?,
                speaker_id,
            })
        })
        .collect()
}

struct Args {
    catalog: PathBuf,
    models: PathBuf,
    stt: PathBuf,
    n: usize,
    out: PathBuf,
    only: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut catalog = None;
    let mut models = None;
    let mut stt = None;
    let mut n = None;
    let mut out = None;
    let mut only = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--catalog" => catalog = Some(PathBuf::from(value("--catalog")?)),
            "--models" => models = Some(PathBuf::from(value("--models")?)),
            "--stt" => stt = Some(PathBuf::from(value("--stt")?)),
            "--n" => {
                n = Some(
                    value("--n")?
                        .parse::<usize>()
                        .map_err(|e| format!("--n: {e}"))?,
                )
            }
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--only" => only.push(value("--only")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Args {
        catalog: catalog.ok_or("missing --catalog")?,
        models: models.ok_or("missing --models")?,
        stt: stt.ok_or("missing --stt")?,
        n: n.filter(|n| *n > 0).ok_or("missing or zero --n")?,
        out: out.ok_or("missing --out")?,
        only,
    })
}

fn num(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{x:.2}"),
        _ => "null".into(),
    }
}

fn write_out(
    path: &Path,
    n: usize,
    models_json: &[String],
    clips_json: &[String],
) -> Result<(), String> {
    let speeds = SPEEDS
        .iter()
        .map(|s| format!("{s}"))
        .collect::<Vec<_>>()
        .join(",");
    let body = format!(
        "{{\"n\":{n},\"speeds\":[{speeds}],\"body\":\"{}\",\"models\":[\n{}\n],\"clips\":[\n{}\n]}}\n",
        json_escape(BODY),
        models_json.join(",\n"),
        clips_json.join(",\n")
    );
    std::fs::write(path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

fn run(args: &Args) -> Result<(), String> {
    let catalog = read_catalog(&args.catalog)?;
    for id in &args.only {
        if !catalog.iter().any(|m| &m.id == id) {
            return Err(format!("--only {id}: not in catalog"));
        }
    }
    let recognizer = create_recognizer(&args.stt)?;
    let lead = vec![0.0_f32; SAMPLE_RATE as usize * LEAD_SILENCE_MS / 1000];
    let tail = vec![0.0_f32; SAMPLE_RATE as usize * TAIL_SILENCE_MS / 1000];

    let mut models_json = Vec::new();
    let mut clips_json = Vec::new();
    let selected: Vec<&CatalogModel> = catalog
        .iter()
        .filter(|m| args.only.is_empty() || args.only.contains(&m.id))
        .collect();
    for m in catalog.iter().filter(|m| m.language != "en") {
        models_json.push(format!(
            "{{\"id\":\"{}\",\"bundle\":\"{}\",\"language\":\"{}\",\"kind\":\"{}\",\"speakerId\":{},\"status\":\"skipped: English openers do not apply\"}}",
            json_escape(&m.id), json_escape(&m.bundle), json_escape(&m.language), json_escape(&m.kind), m.speaker_id
        ));
    }

    // One engine per bundle; speakers of a multi-speaker bundle share it.
    let mut bundles: Vec<&str> = Vec::new();
    for m in selected.iter().filter(|m| m.language == "en") {
        if !bundles.contains(&m.bundle.as_str()) {
            bundles.push(&m.bundle);
        }
    }
    for bundle in bundles {
        let dir = args.models.join(bundle);
        let tts = create_tts(&dir)?;
        for m in selected
            .iter()
            .filter(|m| m.language == "en" && m.bundle == bundle)
        {
            let started = std::time::Instant::now();
            for speed in SPEEDS {
                for opener in OPENERS {
                    for (form, text) in
                        [("written", as_written(opener)), ("merged", merged(opener))]
                    {
                        for iter in 0..args.n {
                            let (raw, rate) = synthesize_at(&tts, &text, m.speaker_id, speed)?;
                            let clip = to_recognizer_rate(raw, rate)?;
                            let metrics = clip_metrics(&clip);
                            let mut input =
                                Vec::with_capacity(lead.len() + clip.len() + tail.len());
                            input.extend_from_slice(&lead);
                            input.extend_from_slice(&clip);
                            input.extend_from_slice(&tail);
                            let transcript = recognize(&recognizer, &input);
                            let first = first_token(&transcript);
                            let ok = first == expected_first_word(opener);
                            let (onset, end, head, body, delta, gap) = match &metrics {
                                Some(x) => (
                                    x.onset_ms.to_string(),
                                    x.end_ms.to_string(),
                                    num(Some(x.head_db)),
                                    num(x.body_db),
                                    num(x.delta_db),
                                    x.gap_ms.to_string(),
                                ),
                                None => (
                                    "null".into(),
                                    "null".into(),
                                    "null".into(),
                                    "null".into(),
                                    "null".into(),
                                    "null".into(),
                                ),
                            };
                            clips_json.push(format!(
                                "{{\"model\":\"{}\",\"speed\":{speed},\"opener\":\"{}\",\"form\":\"{form}\",\"iter\":{iter},\"text\":\"{}\",\"ttsRate\":{rate},\"durationMs\":{},\"onsetMs\":{onset},\"endMs\":{end},\"headDb\":{head},\"bodyDb\":{body},\"deltaDb\":{delta},\"gapMs\":{gap},\"transcript\":\"{}\",\"firstToken\":\"{}\",\"firstWordOk\":{ok}}}",
                                json_escape(&m.id),
                                json_escape(opener),
                                json_escape(&text),
                                clip.len() * 1000 / SAMPLE_RATE as usize,
                                json_escape(&transcript),
                                json_escape(&first),
                            ));
                        }
                    }
                }
            }
            models_json.push(format!(
                "{{\"id\":\"{}\",\"bundle\":\"{}\",\"language\":\"en\",\"kind\":\"{}\",\"speakerId\":{},\"status\":\"swept\"}}",
                json_escape(&m.id), json_escape(&m.bundle), json_escape(&m.kind), m.speaker_id
            ));
            eprintln!(
                "opener_sweep: {} done in {:.0} s",
                m.id,
                started.elapsed().as_secs_f64()
            );
            write_out(&args.out, args.n, &models_json, &clips_json)?;
        }
    }
    write_out(&args.out, args.n, &models_json, &clips_json)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "opener_sweep: {e}\nusage: opener_sweep --catalog <json> --models <dir> --stt <dir> --n <N> --out <json> [--only <model-id>]..."
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = run(&args) {
        eprintln!("opener_sweep: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_form_replaces_bang_and_lowercases_next_word() {
        assert_eq!(merged("Sure!"), "Sure, ask me anything about VoiceThere.");
        assert_eq!(
            merged("Got it!"),
            "Got it, ask me anything about VoiceThere."
        );
        assert_eq!(as_written("Hi!"), "Hi! Ask me anything about VoiceThere.");
    }

    #[test]
    fn first_word_match_folds_ok_and_strips_punctuation() {
        assert_eq!(expected_first_word("OK!"), "okay");
        assert_eq!(expected_first_word("Got it!"), "got");
        assert_eq!(first_token("OK ASK ME"), "okay");
        assert_eq!(first_token(" HI, ask"), "hi");
        assert_eq!(first_token(""), "");
    }

    fn tone(ms: usize, amp: f32) -> Vec<f32> {
        (0..ms * 16)
            .map(|i| amp * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 16_000.0).sin())
            .collect()
    }

    #[test]
    fn quiet_head_and_gap_are_measured() {
        // 100 ms silence, 200 ms quiet head (-20 dB rel), 300 ms gap, 1 s loud body.
        let mut clip = vec![0.0; 1600];
        clip.extend(tone(200, 0.05));
        clip.extend(vec![0.0; 4800]);
        clip.extend(tone(1000, 0.5));
        let m = clip_metrics(&clip).expect("metrics");
        assert_eq!(m.onset_ms, 100);
        assert_eq!(m.end_ms, 1600);
        assert_eq!(m.gap_ms, 300);
        let delta = m.delta_db.expect("delta");
        assert!(delta < -10.0, "delta {delta}");
    }

    #[test]
    fn short_clip_has_no_body() {
        let m = clip_metrics(&tone(300, 0.5)).expect("metrics");
        assert_eq!(m.body_db, None);
        assert!(clip_metrics(&[0.0; 1600]).is_none());
    }
}
