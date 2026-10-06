//! Sentence splitting for TTS scheduling.
//!
//! A TTS request is synthesised one sentence at a time. Each sentence takes the
//! TTS permit and an engine on its own, so a short reply queued behind a long
//! one waits for one sentence instead of the whole text.

/// Upper bound (in chars) for one synthesised piece. The ORT arena grows to the
/// longest input it has seen, so very long inputs are cut at a comma or a space.
pub const MAX_SENTENCE_CHARS: usize = 300;

/// A piece with fewer words than this is merged into its neighbour. espeak reads
/// fragments like `"I. am. happy."` as one sentence, and a lone `"I."` is spoken as a
/// letter, so tiny pieces must not be synthesised on their own.
const MIN_SENTENCE_WORDS: usize = 4;

/// A first piece with more words than this is cut at a comma so the first audio starts sooner.
const FIRST_PIECE_MAX_WORDS: usize = 12;

const ASCII_TERMINATORS: [char; 4] = ['.', '!', '?', ';'];
const CJK_TERMINATORS: [char; 4] = ['。', '！', '？', '；'];

/// Split `text` into sentences for per-sentence synthesis.
///
/// - Splits after `.`, `!`, `?`, `;` when followed by whitespace or the end of the
///   text (so `1.5` stays whole). The CJK terminators `。！？；` split wherever they
///   occur, since CJK text has no space after them.
/// - A piece with no alphanumeric character joins the previous sentence, or the
///   next one when it comes first.
/// - A piece with fewer than [`MIN_SENTENCE_WORDS`] words is merged with the next piece
///   (and a short last piece with the previous one). For text with CJK characters, which
///   has no spaces, a piece with fewer than that many letters or digits counts as short.
/// - A sentence longer than [`MAX_SENTENCE_CHARS`] is cut at the last `,` within the
///   limit, else at the last whitespace within the limit, else hard at the limit.
/// - Empty or whitespace-only text yields an empty vec.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        let is_cjk_end = CJK_TERMINATORS.contains(&c);
        let is_ascii_end = ASCII_TERMINATORS.contains(&c)
            && chars
                .peek()
                .map(|next| next.is_whitespace())
                .unwrap_or(true);
        if is_cjk_end || is_ascii_end {
            push_trimmed(&mut raw, &mut current);
        }
    }
    push_trimmed(&mut raw, &mut current);

    let merged = merge_short(merge_symbol_only(raw));

    let mut out = Vec::with_capacity(merged.len());
    for sentence in merged {
        split_long(&sentence, &mut out);
    }
    out
}

/// Like [`split_sentences`], but keeps the FIRST piece short so first audio starts sooner.
///
/// When the first piece has more than [`FIRST_PIECE_MAX_WORDS`] words and a `,` ends a word
/// after at least [`MIN_SENTENCE_WORDS`] words, the piece is cut at the first such comma (the
/// comma stays with the first part). The cut is skipped when the rest would be shorter than
/// [`MIN_SENTENCE_WORDS`] words, since a tiny piece must not be synthesised on its own.
/// Pieces after the first are unchanged.
pub fn split_for_first_audio(text: &str) -> Vec<String> {
    let mut pieces = split_sentences(text);
    let Some(first) = pieces.first() else {
        return pieces;
    };
    if first.split_whitespace().count() <= FIRST_PIECE_MAX_WORDS {
        return pieces;
    }
    let mut cut: Option<usize> = None;
    for (index, word) in first.split_whitespace().enumerate() {
        if index + 1 >= MIN_SENTENCE_WORDS && word.ends_with(',') {
            // `word` is a subslice of `first`: its end offset is the cut position.
            cut = Some(word.as_ptr() as usize - first.as_ptr() as usize + word.len());
            break;
        }
    }
    let Some(cut) = cut else {
        return pieces;
    };
    let head = first[..cut].trim_end().to_string();
    let rest = first[cut..].trim().to_string();
    if rest.split_whitespace().count() < MIN_SENTENCE_WORDS {
        return pieces;
    }
    pieces[0] = rest;
    pieces.insert(0, head);
    pieces
}

fn push_trimmed(out: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    current.clear();
}

fn has_alphanumeric(piece: &str) -> bool {
    piece.chars().any(char::is_alphanumeric)
}

/// Pieces without letters or digits (`...`, `?!`) carry no speech of their own.
fn merge_symbol_only(pieces: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(pieces.len());
    let mut pending_prefix: Option<String> = None;
    for piece in pieces {
        if has_alphanumeric(&piece) {
            match pending_prefix.take() {
                Some(prefix) => out.push(format!("{prefix} {piece}")),
                None => out.push(piece),
            }
        } else if let Some(previous) = out.last_mut() {
            previous.push(' ');
            previous.push_str(&piece);
        } else {
            pending_prefix = Some(match pending_prefix.take() {
                Some(prefix) => format!("{prefix} {piece}"),
                None => piece,
            });
        }
    }
    // Text that is only symbols: nothing to say.
    out
}

fn is_cjk_letter(c: char) -> bool {
    matches!(
        c,
        '\u{3040}'..='\u{30FF}' // hiragana, katakana
            | '\u{3400}'..='\u{9FFF}' // CJK ideographs
            | '\u{AC00}'..='\u{D7AF}' // hangul syllables
            | '\u{F900}'..='\u{FAFF}' // CJK compatibility ideographs
    )
}

/// Too small to synthesise on its own.
fn is_short(piece: &str) -> bool {
    if piece.chars().any(is_cjk_letter) {
        piece.chars().filter(|c| c.is_alphanumeric()).count() < MIN_SENTENCE_WORDS
    } else {
        piece.split_whitespace().count() < MIN_SENTENCE_WORDS
    }
}

/// Join two pieces with one space, or none after CJK text (which has no spaces).
fn join_pieces(first: &str, second: &str) -> String {
    let no_space = first
        .chars()
        .last()
        .is_some_and(|c| is_cjk_letter(c) || CJK_TERMINATORS.contains(&c));
    if no_space {
        format!("{first}{second}")
    } else {
        format!("{first} {second}")
    }
}

/// Merge pieces smaller than [`MIN_SENTENCE_WORDS`] forward into the next piece. A short
/// piece left at the end joins the previous one.
fn merge_short(pieces: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(pieces.len());
    let mut current: Option<String> = None;
    for piece in pieces {
        let combined = match current.take() {
            Some(head) => join_pieces(&head, &piece),
            None => piece,
        };
        if is_short(&combined) {
            current = Some(combined);
        } else {
            out.push(combined);
        }
    }
    if let Some(tail) = current {
        match out.last_mut() {
            Some(previous) => *previous = join_pieces(previous, &tail),
            None => out.push(tail),
        }
    }
    out
}

fn split_long(sentence: &str, out: &mut Vec<String>) {
    let mut rest: Vec<char> = sentence.chars().collect();
    while rest.len() > MAX_SENTENCE_CHARS {
        // Comma within the first MAX chars (kept on the first piece).
        let comma = rest[..MAX_SENTENCE_CHARS].iter().rposition(|&c| c == ',');
        // Whitespace within the first MAX + 1 chars (dropped), so a space right
        // after a full-length piece is still a clean cut.
        let space = rest[..=MAX_SENTENCE_CHARS]
            .iter()
            .rposition(|c| c.is_whitespace())
            .filter(|&index| index > 0);
        let (head_len, skip) = match (comma, space) {
            (Some(index), _) => (index + 1, 0),
            (None, Some(index)) => (index, 1),
            (None, None) => (MAX_SENTENCE_CHARS, 0),
        };
        let head: String = rest[..head_len].iter().collect();
        let head = head.trim();
        if !head.is_empty() {
            out.push(head.to_string());
        }
        rest.drain(..head_len + skip);
        let leading = rest.iter().take_while(|c| c.is_whitespace()).count();
        rest.drain(..leading);
    }
    let tail: String = rest.iter().collect();
    let tail = tail.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pieces_merge_into_one_sentence() {
        // "Hello there." is short and merges forward; "Fine!" is short and merges back.
        assert_eq!(
            split_sentences("Hello there. How are you? Fine!"),
            vec!["Hello there. How are you? Fine!"]
        );
    }

    #[test]
    fn splits_sentences_of_four_or_more_words() {
        assert_eq!(
            split_sentences("Hello there my friend. How are you doing today? Fine!"),
            vec!["Hello there my friend.", "How are you doing today? Fine!"]
        );
    }

    #[test]
    fn one_word_fragments_merge_into_one_sentence() {
        assert_eq!(split_sentences("I. am. happy."), vec!["I. am. happy."]);
    }

    #[test]
    fn short_first_piece_merges_forward() {
        assert_eq!(
            split_sentences("Yes. I can do that for you right now."),
            vec!["Yes. I can do that for you right now."]
        );
    }

    #[test]
    fn semicolon_splits() {
        assert_eq!(
            split_sentences("First part is done here; second part is still to come."),
            vec!["First part is done here;", "second part is still to come."]
        );
    }

    #[test]
    fn does_not_split_inside_decimal_number() {
        assert_eq!(
            split_sentences("Version 1.5 is out."),
            vec!["Version 1.5 is out."]
        );
    }

    #[test]
    fn symbol_only_text_yields_no_sentences() {
        assert!(split_sentences("...").is_empty());
    }

    #[test]
    fn symbol_only_piece_joins_previous_sentence() {
        assert_eq!(
            split_sentences("Well that is quite something. ... Then we go home now."),
            vec!["Well that is quite something. ...", "Then we go home now."]
        );
    }

    #[test]
    fn symbol_only_first_piece_joins_next_sentence() {
        assert_eq!(split_sentences("... Then we go."), vec!["... Then we go."]);
    }

    #[test]
    fn ellipsis_inside_sentence_does_not_split_early() {
        assert_eq!(
            split_sentences("Wait for it to finish... what a surprise this is? Okay."),
            vec!["Wait for it to finish...", "what a surprise this is? Okay."]
        );
    }

    #[test]
    fn long_text_without_punctuation_is_cut_at_whitespace() {
        let words: Vec<String> = (0..90).map(|i| format!("word{i:02}")).collect();
        let text = words.join(" ");
        assert!(text.chars().count() > MAX_SENTENCE_CHARS);
        let pieces = split_sentences(&text);
        assert!(pieces.len() > 1);
        for piece in &pieces {
            assert!(
                piece.chars().count() <= MAX_SENTENCE_CHARS,
                "piece too long: {}",
                piece.chars().count()
            );
        }
        assert_eq!(pieces.join(" "), text);
    }

    #[test]
    fn long_sentence_prefers_last_comma_before_limit() {
        let first = format!("{},", "a".repeat(150));
        let second = format!("{}, ", "b".repeat(100));
        let third = "c".repeat(100);
        let text = format!("{first} {second}{third}");
        let pieces = split_sentences(&text);
        // First 300 chars contain two commas; the cut is at the later one.
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0], format!("{first} {}", second.trim_end()));
        assert_eq!(pieces[1], third);
    }

    #[test]
    fn long_word_without_any_break_is_cut_hard() {
        let text = "x".repeat(MAX_SENTENCE_CHARS * 2 + 10);
        let pieces = split_sentences(&text);
        assert_eq!(pieces.len(), 3);
        assert!(pieces
            .iter()
            .all(|piece| piece.chars().count() <= MAX_SENTENCE_CHARS));
        assert_eq!(pieces.concat(), text);
    }

    #[test]
    fn short_cjk_pieces_merge() {
        // Both pieces have fewer than four letters, so they merge (no space inserted).
        assert_eq!(split_sentences("你好。再见！"), vec!["你好。再见！"]);
    }

    #[test]
    fn cjk_pieces_of_four_or_more_letters_split() {
        assert_eq!(
            split_sentences("今天天气很好。我们去公园散步吧！"),
            vec!["今天天气很好。", "我们去公园散步吧！"]
        );
    }

    #[test]
    fn trailing_and_leading_whitespace_is_trimmed() {
        assert_eq!(
            split_sentences("  Hello there my friend.   How are you doing today?  "),
            vec!["Hello there my friend.", "How are you doing today?"]
        );
        assert_eq!(split_sentences("One. "), vec!["One."]);
    }

    #[test]
    fn empty_and_whitespace_text_yield_nothing() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n\t ").is_empty());
    }

    #[test]
    fn text_without_terminator_is_one_sentence() {
        assert_eq!(split_sentences("no terminator"), vec!["no terminator"]);
    }

    const LONG_FIRST: &str =
        "Well, I checked the calendar for next week, and Tuesday at ten works for everyone on the team.";

    #[test]
    fn first_piece_cut_at_comma_when_long() {
        // "Well," follows only one word, so the first usable comma is after "week," (8 words).
        assert_eq!(
            split_for_first_audio(LONG_FIRST),
            vec![
                "Well, I checked the calendar for next week,",
                "and Tuesday at ten works for everyone on the team."
            ]
        );
    }

    #[test]
    fn first_piece_short_unchanged() {
        let text = "Sure, I can do that for you. Then we will go home now.";
        assert_eq!(split_for_first_audio(text), split_sentences(text));
    }

    #[test]
    fn later_pieces_unchanged() {
        let text =
            format!("{LONG_FIRST} Then, we will all go out for lunch together, if that suits you.");
        let pieces = split_for_first_audio(&text);
        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[2], split_sentences(&text)[1]);
    }

    #[test]
    fn no_comma_unchanged() {
        let text = "I checked the calendar for next week and Tuesday at ten works for everyone.";
        assert!(text.split_whitespace().count() > FIRST_PIECE_MAX_WORDS);
        assert_eq!(split_for_first_audio(text), split_sentences(text));
    }

    #[test]
    fn cut_skipped_when_rest_would_be_tiny() {
        let text =
            "One two three four five six seven eight nine ten eleven twelve thirteen, yes ok.";
        assert_eq!(split_for_first_audio(text), split_sentences(text));
    }

    #[test]
    fn empty_text_yields_nothing() {
        assert!(split_for_first_audio("  ").is_empty());
    }
}
