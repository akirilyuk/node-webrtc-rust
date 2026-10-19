//! Audio and transcript helpers shared by the live tests.

/// The phrase every STT live test speaks (and the fixture contains).
pub const STT_PHRASE: &str = "The quick brown fox jumps over the lazy dog.";
/// [`STT_PHRASE`] as lowercase words.
pub const STT_WORDS: [&str; 9] = [
    "the", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
];

/// The phrase of the full-stack STT tests: about 5 s of speech, long enough that vendors
/// finalize segments before the speaker is done.
pub const COUNT_PHRASE: &str = "one. two. three. four. five. six. seven. eight. nine. ten.";
/// [`COUNT_PHRASE`] as lowercase words.
pub const COUNT_WORDS: [&str; 10] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
];

/// RMS of s16le samples. `0.0` for an empty buffer.
pub fn rms(pcm: &[u8]) -> f64 {
    let samples: Vec<i16> = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt()
}

/// How many of `expected` appear in `transcript` (lowercase, punctuation stripped, each
/// transcript word used once).
pub fn matched_words(transcript: &str, expected: &[&str]) -> usize {
    let cleaned: String = transcript
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let mut heard: Vec<&str> = cleaned.split_whitespace().collect();
    let mut matched = 0;
    for word in expected {
        if let Some(at) = heard.iter().position(|h| h == word) {
            heard.swap_remove(at);
            matched += 1;
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_phrase_matches_all_nine() {
        assert_eq!(matched_words(STT_PHRASE, &STT_WORDS), 9);
        assert_eq!(
            matched_words(
                "THE QUICK, brown fox... jumps over the lazy dog!",
                &STT_WORDS
            ),
            9
        );
    }

    #[test]
    fn each_heard_word_is_used_once() {
        // One "the" in the transcript can only match one of the two expected "the".
        assert_eq!(matched_words("the quick brown fox", &STT_WORDS), 4);
        assert_eq!(matched_words("the the the", &STT_WORDS), 2);
        assert_eq!(matched_words("", &STT_WORDS), 0);
    }

    #[test]
    fn rms_of_silence_constant_and_empty() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0; 64]), 0.0);
        let pcm: Vec<u8> = (0..32).flat_map(|_| 1000i16.to_le_bytes()).collect();
        assert!((rms(&pcm) - 1000.0).abs() < 1e-9);
    }
}
