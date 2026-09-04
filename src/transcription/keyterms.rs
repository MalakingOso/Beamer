//! Keyterm ("custom vocabulary") preparation for the ElevenLabs backends.
//!
//! Every rule below rejects the whole request rather than the offending term,
//! so `sanitize` drops bad terms silently: a dropped term degrades one
//! dictation, a 400 loses it entirely.

/// Longest term each endpoint accepts, inclusive. Batch is 49, not 50: the API
/// documents "less than 50 characters".
pub const BATCH_MAX_CHARS: usize = 49;
/// Realtime documents "a maximum length of 20 characters" — inclusive.
pub const REALTIME_MAX_CHARS: usize = 20;

/// How many terms each endpoint takes. Batch is capped at 100, not the
/// documented 1000: above 100 ElevenLabs bills a 20-second minimum per request.
pub const BATCH_MAX_TERMS: usize = 100;
/// Realtime's hard ceiling.
pub const REALTIME_MAX_TERMS: usize = 50;

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

/// Percent-encode one keyterm for use as a query-string value. Conservative:
/// everything outside the RFC 3986 unreserved set is escaped.
pub fn encode_query_value(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for byte in term.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
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
    fn ordinary_terms_pass_through_unchanged() {
        let input = terms(&["Beamer", "Deploy Purple", "Dioxus"]);
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            ["Beamer", "Deploy Purple", "Dioxus"]
        );
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
    fn over_long_terms_are_dropped_not_truncated() {
        let input = terms(&["ok", &"x".repeat(60)]);
        assert_eq!(sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS), ["ok"]);
    }

    #[test]
    fn the_length_limit_is_inclusive_at_each_endpoints_maximum() {
        let batch_edge = "x".repeat(BATCH_MAX_CHARS);
        let batch_over = "x".repeat(BATCH_MAX_CHARS + 1);
        let input = terms(&[&batch_edge, &batch_over]);
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS),
            [batch_edge]
        );

        let rt_edge = "y".repeat(REALTIME_MAX_CHARS);
        let rt_over = "y".repeat(REALTIME_MAX_CHARS + 1);
        let input = terms(&[&rt_edge, &rt_over]);
        assert_eq!(
            sanitize(&input, REALTIME_MAX_TERMS, REALTIME_MAX_CHARS),
            [rt_edge]
        );
    }

    /// `"é".repeat(20)` is 40 bytes but 20 characters, and legal at realtime's limit.
    #[test]
    fn length_is_counted_in_characters_not_bytes() {
        let accented = "é".repeat(REALTIME_MAX_CHARS);
        assert_eq!(accented.len(), REALTIME_MAX_CHARS * 2);
        assert_eq!(
            sanitize(
                &terms(&[&accented]),
                REALTIME_MAX_TERMS,
                REALTIME_MAX_CHARS
            ),
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

    #[test]
    fn the_count_cap_keeps_the_first_terms_in_order() {
        let input: Vec<String> = (0..120).map(|i| format!("term{i}")).collect();
        let out = sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS);
        assert_eq!(out.len(), BATCH_MAX_TERMS);
        assert_eq!(out[0], "term0");
        assert_eq!(out[BATCH_MAX_TERMS - 1], format!("term{}", BATCH_MAX_TERMS - 1));
    }

    /// The cap counts surviving terms, not offered ones.
    #[test]
    fn rejected_terms_do_not_count_against_the_cap() {
        let mut input = terms(&["a<b", "c>d"]);
        input.extend((0..REALTIME_MAX_TERMS).map(|i| format!("t{i}")));
        let out = sanitize(&input, REALTIME_MAX_TERMS, REALTIME_MAX_CHARS);
        assert_eq!(out.len(), REALTIME_MAX_TERMS);
        assert_eq!(out[0], "t0");
    }

    #[test]
    fn realtime_takes_a_tighter_budget_than_batch() {
        let input: Vec<String> = (0..80).map(|i| format!("t{i}")).collect();
        assert_eq!(
            sanitize(&input, REALTIME_MAX_TERMS, REALTIME_MAX_CHARS).len(),
            REALTIME_MAX_TERMS
        );
        assert_eq!(
            sanitize(&input, BATCH_MAX_TERMS, BATCH_MAX_CHARS).len(),
            80
        );
    }

    #[test]
    fn unreserved_characters_are_left_alone() {
        assert_eq!(encode_query_value("Beamer-v1.0_x~y"), "Beamer-v1.0_x~y");
    }

    /// A raw space would break the WebSocket upgrade URL.
    #[test]
    fn spaces_and_separators_are_escaped() {
        assert_eq!(encode_query_value("Deploy Purple"), "Deploy%20Purple");
        assert_eq!(encode_query_value("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_query_value("a+b"), "a%2Bb");
        assert_eq!(encode_query_value("50%"), "50%25");
    }

    #[test]
    fn multi_byte_characters_are_escaped_per_utf8_byte() {
        assert_eq!(encode_query_value("é"), "%C3%A9");
        assert_eq!(encode_query_value("日本"), "%E6%97%A5%E6%9C%AC");
    }
}
