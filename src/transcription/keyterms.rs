//! Keyterm ("custom vocabulary") preparation for the ElevenLabs batch backends.
//!
//! Every rule below rejects the whole request rather than the offending term,
//! so `sanitize` drops bad terms silently: a dropped term degrades one
//! dictation, a 400 loses it entirely.

/// Longest term the batch endpoint accepts, inclusive. Batch is 49, not 50:
/// the API documents "less than 50 characters".
pub const BATCH_MAX_CHARS: usize = 49;

/// How many terms the batch endpoint takes. Capped at 100, not the
/// documented 1000: above 100 ElevenLabs bills a 20-second minimum per request.
pub const BATCH_MAX_TERMS: usize = 100;

/// A keyterm may contain at most this many words after normalisation.
const MAX_WORDS: usize = 5;

/// Characters the API explicitly does not support inside a keyterm.
const FORBIDDEN: [char; 7] = ['<', '>', '{', '}', '[', ']', '\\'];

/// Trim, drop the terms the API would reject, de-duplicate, and cap the count.
/// Order is preserved, so the list's own ordering decides what survives the cap.
pub fn sanitize(terms: &[String], max_terms: usize, max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for term in terms {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        // Characters, not bytes: the API counts characters.
        if term.chars().count() > max_chars {
            continue;
        }
        if term.split_whitespace().count() > MAX_WORDS {
            continue;
        }
        if term.contains(FORBIDDEN) {
            continue;
        }
        if out.iter().any(|existing| existing == term) {
            continue;
        }
        out.push(term.to_string());
        if out.len() == max_terms {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_and_blanks_dropped() {
        let input = terms(&["  Beamer  ", "   ", "", "\tScribe\n"]);
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            ["Beamer", "Scribe"]
        );
    }

    /// An over-long term fails the whole request, so it must never reach the API.
    #[test]
    fn the_length_limit_is_inclusive_at_the_maximum() {
        let edge = "x".repeat(BATCH_MAX_CHARS);
        let over = "x".repeat(BATCH_MAX_CHARS + 1);
        let input = terms(&[&edge, &over]);
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            [edge]
        );
    }

    /// `"é".repeat(49)` is 98 bytes but 49 characters, and legal.
    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        let accented = "é".repeat(BATCH_MAX_CHARS);
        assert_eq!(accented.len(), BATCH_MAX_CHARS * 2);
        assert_eq!(
            sanitize(&terms(&[&accented]), BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            [accented]
        );
    }

    #[test]
    fn terms_of_more_than_five_words_are_dropped() {
        let input = terms(&[
            "one two three four five",
            "one two three four five six",
        ]);
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            ["one two three four five"]
        );
    }

    #[test]
    fn terms_containing_unsupported_characters_are_dropped() {
        let input = terms(&["fine", "a<b", "a>b", "a{b", "a}b", "a[b", "a]b", "a\\b"]);
        assert_eq!(sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS), ["fine"]);
    }

    /// Entries differing only in surrounding whitespace collapse to one term.
    #[test]
    fn duplicates_that_appear_after_trimming_are_collapsed() {
        let input = terms(&["Beamer", " Beamer ", "beamer"]);
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            ["Beamer", "beamer"],
            "de-duplication is exact, so a different case is a different term"
        );
    }

    /// The cap counts surviving terms, not offered ones.
    #[test]
    fn rejected_terms_do_not_count_against_the_cap() {
        let mut input = terms(&["a<b", "c>d"]);
        input.extend((0..BATCH_MAX_TERMS).map(|i| format!("t{i}")));
        let out = sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS);
        assert_eq!(out.len(), BATCH_MAX_TERMS);
        assert_eq!(out[0], "t0");
    }
}
