//! Qualifies a roundtrip phrase against the local Sherpa recognizer: synthesizes it N times with
//! nwr's Piper settings (the output differs per run), recognizes each clip directly with the voice
//! agent's online recognizer (no WebRTC) and reports how close the transcripts are to the phrase.
//!
//! ```text
//! cargo run --release -p node-webrtc-rust-vendor-sherpa-onnx --example phrase_sweep -- \
//!   --stt <stt-model-dir> --tts <tts-model-dir> --n <N> [--keyword <word>]... "<phrase>"
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
//! Output: one JSON object on stdout; progress goes to stderr.

use std::collections::HashMap;
use std::path::PathBuf;

#[path = "support/mod.rs"]
mod support;

use support::{create_recognizer, create_tts, recognize, synthesize};

const LEAD_SILENCE_MS: usize = 300;
const TAIL_SILENCE_MS: usize = 2500;
const SAMPLE_RATE: i32 = 16_000;

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
    phrase: String,
}

fn parse_args() -> Result<Args, String> {
    let mut stt = None;
    let mut tts = None;
    let mut n = None;
    let mut keywords = Vec::new();
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
        phrase: phrase.ok_or("missing phrase")?,
    })
}

fn run(args: &Args) -> Result<String, String> {
    let recognizer = create_recognizer(&args.stt)?;
    let tts = create_tts(&args.tts)?;
    let lead = vec![0.0_f32; SAMPLE_RATE as usize * LEAD_SILENCE_MS / 1000];
    let tail = vec![0.0_f32; SAMPLE_RATE as usize * TAIL_SILENCE_MS / 1000];

    let mut min_observed = f64::MAX;
    let mut below_075 = 0usize;
    let mut below_060 = 0usize;
    let mut keyword_missing = 0usize;
    let mut texts: HashMap<String, usize> = HashMap::new();

    for i in 0..args.n {
        let (clip, rate) = synthesize(&tts, &args.phrase)?;
        if rate != SAMPLE_RATE {
            return Err(format!(
                "TTS sample rate is {rate} Hz, expected {SAMPLE_RATE} Hz (amy-low)"
            ));
        }
        let mut input = Vec::with_capacity(lead.len() + clip.len() + tail.len());
        input.extend_from_slice(&lead);
        input.extend_from_slice(&clip);
        input.extend_from_slice(&tail);
        let text = recognize(&recognizer, &input);

        let similarity = word_similarity(&args.phrase, &text);
        min_observed = min_observed.min(similarity);
        if similarity < 0.75 {
            below_075 += 1;
        }
        if similarity < 0.60 {
            below_060 += 1;
        }
        if !all_keywords_are_tokens(&text, &args.keywords) {
            keyword_missing += 1;
        }
        *texts.entry(text).or_insert(0) += 1;
        if (i + 1) % 50 == 0 {
            eprintln!("phrase_sweep: {}/{}", i + 1, args.n);
        }
    }

    let mut top: Vec<(String, usize)> = texts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(5);
    let top_json = top
        .iter()
        .map(|(text, count)| format!("[{},{count}]", json_string(text)))
        .collect::<Vec<_>>()
        .join(",");
    let (keyword_json, missing_json) = if args.keywords.is_empty() {
        ("null".to_string(), "null".to_string())
    } else {
        (
            json_string(&args.keywords.join(",")),
            keyword_missing.to_string(),
        )
    };
    Ok(format!(
        "{{\"phrase\":{},\"n\":{},\"minObserved\":{},\"below075\":{below_075},\"below060\":{below_060},\"keyword\":{keyword_json},\"keywordMissing\":{missing_json},\"topTexts\":[{top_json}]}}",
        json_string(&args.phrase),
        args.n,
        min_observed,
    ))
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "phrase_sweep: {e}\nusage: phrase_sweep --stt <dir> --tts <dir> --n <N> [--keyword <word>]... \"<phrase>\""
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
