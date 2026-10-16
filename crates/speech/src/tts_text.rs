//! The text a TTS engine receives for a reply: sentence-start capitalization.
//!
//! Lives in the speech crate so the Sherpa vendor, the cluster speech path and the SDK trace
//! helper (`ttsEngineText`) all compute the same text and the same count.

use std::borrow::Cow;

use crate::config::SentenceCase;

/// Engine input text plus the number of letters uppercased to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineText<'a> {
    pub text: Cow<'a, str>,
    /// Letters uppercased relative to the caller's text (0 for [`SentenceCase::Off`]).
    pub adjusted: u32,
}

/// Abbreviations (lowercased, without the trailing `.`) that end in `.` without ending a sentence.
const NON_TERMINAL_ABBREVIATIONS: [&str; 7] = ["mr", "mrs", "ms", "dr", "st", "vs", "etc"];

/// The text handed to the TTS engine, and how many letters were uppercased to get it.
///
/// Sentence-start capitalization (see [`SentenceCase`]): uppercase the first letter after a sentence end.
///
/// STT finals often start lowercase (`Okay. eight one`). espeak-ng, inside the Piper/VITS
/// engine, does not end a sentence at `. e`, so the engine renders one sentence: late first
/// audio and no pause after `Okay.`. With `. E` it renders two.
///
/// - After `.`, `!` or `?` followed by one or more whitespace chars, the next char is
///   uppercased when it is lowercase (`char::to_uppercase`, which may yield several chars).
/// - Not for an ellipsis (`..` before the terminating `.`), for a token that contains another
///   `.` (`e.g.`, `i.e.`, `U.S.`), or for `mr`, `mrs`, `ms`, `dr`, `st`, `vs`, `etc`.
/// - Not after a single-letter token (`I.`, `J.`, `a.`), and not before a fragment of fewer
///   than 2 words (text up to and including the next `.`, `!`, `?`, or the end). These tiny
///   pieces stay merged in `split_sentences` (in the Sherpa vendor crate), and a capital would make espeak split them
///   again (`I. Am. Happy.` is heard as `Ing have been`).
/// - The first char of the text is never changed, and `1.5` is untouched (no whitespace
///   follows the `.`).
/// - `adjusted` is the number of letters uppercased. [`SentenceCase::Off`] borrows `text` and
///   reports 0.
pub fn prepare_engine_text(text: &str, mode: SentenceCase) -> EngineText<'_> {
    if mode == SentenceCase::Off {
        return EngineText {
            text: Cow::Borrowed(text),
            adjusted: 0,
        };
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut adjusted = 0u32;
    // Index of the char to uppercase, once the loop reaches it.
    let mut upper_at: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        if upper_at == Some(i) {
            out.extend(c.to_uppercase());
            adjusted += 1;
            upper_at = None;
        } else {
            out.push(c);
        }
        if !matches!(c, '.' | '!' | '?') || !ends_sentence_at(&chars, i) {
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j > i + 1
            && j < chars.len()
            && chars[j].is_lowercase()
            && !is_single_letter_token(&chars, i)
            && fragment_word_count(&chars, j) >= MIN_CAPITALIZED_FRAGMENT_WORDS
        {
            upper_at = Some(j);
        }
    }
    EngineText {
        text: Cow::Owned(out),
        adjusted,
    }
}

/// A following fragment needs at least this many words to get a capital (see
/// [`prepare_engine_text`]).
const MIN_CAPITALIZED_FRAGMENT_WORDS: usize = 2;

/// Whether the token that ends at the terminator `chars[i]` (terminator excluded) is one letter.
fn is_single_letter_token(chars: &[char], i: usize) -> bool {
    let start = chars[..i]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |p| p + 1);
    i - start == 1 && chars[start].is_alphabetic()
}

/// Words in `chars[from..]` up to and including the next `.`, `!` or `?` (or the end).
fn fragment_word_count(chars: &[char], from: usize) -> usize {
    let end = chars[from..]
        .iter()
        .position(|c| matches!(c, '.' | '!' | '?'))
        .map_or(chars.len(), |p| from + p + 1);
    chars[from..end]
        .iter()
        .collect::<String>()
        .split_whitespace()
        .count()
}

/// Whether the terminator at `chars[i]` can end a sentence (see [`prepare_engine_text`]).
fn ends_sentence_at(chars: &[char], i: usize) -> bool {
    if chars[i] != '.' {
        return true;
    }
    if i > 0 && chars[i - 1] == '.' {
        return false;
    }
    let start = chars[..i]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |p| p + 1);
    let token: String = chars[start..i].iter().collect();
    if token.contains('.') {
        return false;
    }
    !NON_TERMINAL_ABBREVIATIONS.contains(&token.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capitalize(text: &str) -> String {
        prepare_engine_text(text, SentenceCase::Auto)
            .text
            .into_owned()
    }

    #[test]
    fn capitalizes_first_letter_after_sentence_end() {
        let cases = [
            ("okay. eight one", "okay. Eight one"),
            ("Okay. eight one", "Okay. Eight one"),
            (
                "Okay. eight one six three ten",
                "Okay. Eight one six three ten",
            ),
            ("Stop. now please", "Stop. Now please"),
            ("ab.  cd ef", "ab.  Cd ef"),
            ("привет. мир тут", "привет. Мир тут"),
        ];
        for (input, want) in cases {
            assert_eq!(capitalize(input), want, "input {input:?}");
        }
    }

    #[test]
    fn leaves_text_without_a_lowercase_sentence_start_unchanged() {
        for input in [
            "wait... then",
            "1.5 apples",
            "e.g. this",
            "i.e. that",
            "Dr. smith",
            "vs. them",
            "Okay. Eight",
            "end.",
            "你好。世界",
            "eight one",
        ] {
            assert_eq!(capitalize(input), input, "input {input:?}");
        }
    }

    #[test]
    fn single_letter_token_before_terminator_is_not_a_sentence_end() {
        for input in [
            "I. am. happy.",
            "J. smith said hello",
            "a.  b c",
            "I. am here today",
        ] {
            assert_eq!(capitalize(input), input, "input {input:?}");
        }
    }

    #[test]
    fn fragment_of_fewer_than_two_words_is_not_capitalized() {
        for input in [
            "really? yes",
            "hi! there",
            "okay. eight",
            "привет. мир",
            "Okay. eight.",
        ] {
            assert_eq!(capitalize(input), input, "input {input:?}");
        }
    }

    #[test]
    fn tiny_fragments_stay_merged_for_espeak() {
        // The Sherpa `split_sentences` keeps `I. am. happy.` as one piece because espeak reads it
        // as one sentence (pinned in `sentences.rs`). A capital after each `.` would make espeak
        // split it into three again.
        assert_eq!(capitalize("I. am. happy."), "I. am. happy.");
    }

    #[test]
    fn multi_char_uppercase_is_applied() {
        assert_eq!(capitalize("ok. ß and more"), "ok. SS and more");
    }

    #[test]
    fn adjusted_counts_the_uppercased_letters() {
        let cases = [
            ("Okay. eight one six three ten", 1),
            ("I. am. happy.", 0),
            ("Okay. eight one. nine two three", 2),
            ("okay. eight one", 1),
            ("Okay. Eight", 0),
            ("eight one", 0),
            ("", 0),
            ("ok. ß and more", 1),
        ];
        for (input, want) in cases {
            let got = prepare_engine_text(input, SentenceCase::Auto);
            assert_eq!(got.adjusted, want, "input {input:?}");
        }
    }

    #[test]
    fn auto_returns_the_capitalized_text_with_count() {
        let got = prepare_engine_text("Okay. eight one six three ten", SentenceCase::Auto);
        assert_eq!(got.text, "Okay. Eight one six three ten");
        assert_eq!(got.adjusted, 1);
    }

    #[test]
    fn off_borrows_the_text_and_reports_zero() {
        let got = prepare_engine_text("Okay. eight one six three ten", SentenceCase::Off);
        assert_eq!(got.text, "Okay. eight one six three ten");
        assert_eq!(got.adjusted, 0);
        assert!(matches!(got.text, Cow::Borrowed(_)), "Off must not copy");
    }
}
