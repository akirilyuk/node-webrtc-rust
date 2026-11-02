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

/// Longest opener (in letters) that [`merge_short_opener`] merges into the next sentence.
pub const MAX_OPENER_LETTERS: usize = 6;

/// Merges a lone short opener into the sentence after it: `Hi! Ask me anything.` becomes
/// `Hi, ask me anything.`.
///
/// Kitten nano renders a one-word opener about 18 dB below the sentence after it, with a long
/// near-silent gap, and the recognizer then drops the word. Measurements and the decision to
/// apply this to Kitten models only: `docs/tts-opener-sweep-2026-10-31.md`.
///
/// The text is returned borrowed unless all of these hold:
/// - the first whitespace-separated token is 2 to [`MAX_OPENER_LETTERS`] ASCII letters followed by
///   exactly one `!` or `.` (so `Hi!!`, `Hi...`, `Hi?`, `Hi?!`, `Got it!`, `5!` and `Да!` do not
///   match);
/// - the word is not one of the abbreviations that end in `.` without ending a sentence
///   (`Dr.`, `Mr.`, `Etc.`);
/// - whitespace and then text with at least one letter or digit follow.
///
/// The word keeps its case; the rest follows after `, `. The first letter of the rest is
/// lowercased only when its first word is an ASCII capitalized word of 2 or more letters
/// (`Ask`), so `VoiceThere`, `NASA`, `I` and `I'm` keep their case.
pub fn merge_short_opener(text: &str) -> Cow<'_, str> {
    let Some(token_end) = text.find(char::is_whitespace) else {
        return Cow::Borrowed(text);
    };
    let token = &text[..token_end];
    let Some(word) = token.strip_suffix(['!', '.']) else {
        return Cow::Borrowed(text);
    };
    let letters = word.chars().count();
    let is_opener = (2..=MAX_OPENER_LETTERS).contains(&letters)
        && word.chars().all(|c| c.is_ascii_alphabetic())
        && !NON_TERMINAL_ABBREVIATIONS.contains(&word.to_lowercase().as_str());
    if !is_opener {
        return Cow::Borrowed(text);
    }
    let rest = text[token_end..].trim_start();
    if !rest.chars().any(char::is_alphanumeric) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 1);
    out.push_str(word);
    out.push_str(", ");
    if starts_with_capitalized_word(rest) {
        let mut chars = rest.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_lowercase());
        }
        out.push_str(chars.as_str());
    } else {
        out.push_str(rest);
    }
    Cow::Owned(out)
}

/// Whether the first word of `rest` is an ASCII capitalized word of 2 or more letters
/// (`Ask`, not `I`, `I'm`, `NASA`, `VoiceThere`).
fn starts_with_capitalized_word(rest: &str) -> bool {
    let word: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
    let mut chars = word.chars();
    let (Some(first), true) = (chars.next(), word.len() >= 2) else {
        return false;
    };
    // A non-ASCII letter right after the word means the word is longer than it looks.
    let next = rest[word.len()..].chars().next();
    first.is_ascii_uppercase()
        && chars.all(|c| c.is_ascii_lowercase())
        && !next.is_some_and(char::is_alphabetic)
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

    #[test]
    fn merges_every_short_opener() {
        for opener in [
            "Hi", "Hey", "Sure", "Yes", "No", "Okay", "OK", "Great", "Thanks", "Right", "Hello",
            "Well", "So", "Oh", "Wow", "Yep", "Nope", "Cool", "Done",
        ] {
            let input = format!("{opener}! Ask me anything about VoiceThere.");
            assert_eq!(
                merge_short_opener(&input),
                format!("{opener}, ask me anything about VoiceThere."),
                "{opener}"
            );
        }
        assert_eq!(merge_short_opener("Hi. Ask me."), "Hi, ask me.");
    }

    #[test]
    fn two_word_opener_is_unchanged() {
        for text in ["Got it! Ask me anything.", "Hello there! Ask me anything."] {
            assert!(
                matches!(merge_short_opener(text), Cow::Borrowed(_)),
                "{text}"
            );
        }
    }

    #[test]
    fn single_letter_numbers_and_abbreviations_are_unchanged() {
        for text in [
            "I. Am here.",
            "A! Go now.",
            "5! Ask me.",
            "Dr. Smith is here.",
            "Mr. Jones said hi.",
            "Etc. And more.",
        ] {
            assert!(
                matches!(merge_short_opener(text), Cow::Borrowed(_)),
                "{text}"
            );
        }
    }

    #[test]
    fn seven_letters_is_unchanged() {
        assert!(matches!(
            merge_short_opener("Awesome! Ask me."),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn lone_opener_stays() {
        for text in ["Hi!", "Hi! ", "Hi! !!"] {
            assert!(
                matches!(merge_short_opener(text), Cow::Borrowed(_)),
                "{text:?}"
            );
        }
    }

    #[test]
    fn non_english_text_is_unaffected() {
        for text in ["Да! Спросите меня.", "你好！请问。", "Ça! Va bien."] {
            assert!(
                matches!(merge_short_opener(text), Cow::Borrowed(_)),
                "{text}"
            );
        }
    }

    #[test]
    fn merge_keeps_the_case_of_names_and_i() {
        assert_eq!(
            merge_short_opener("Hi! VoiceThere is here."),
            "Hi, VoiceThere is here."
        );
        assert_eq!(merge_short_opener("Hi! I can help."), "Hi, I can help.");
        assert_eq!(merge_short_opener("Hi! I'm here."), "Hi, I'm here.");
        assert_eq!(merge_short_opener("Hi! NASA called."), "Hi, NASA called.");
    }

    #[test]
    fn terminator_variants_are_unchanged() {
        for text in [
            "Hi!! Ask me.",
            "Hi?! Ask me.",
            "Hi... Ask me.",
            "Hi? Ask me.",
        ] {
            assert!(
                matches!(merge_short_opener(text), Cow::Borrowed(_)),
                "{text}"
            );
        }
    }

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
