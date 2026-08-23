//! Keyterm ("custom vocabulary") preparation for the two ElevenLabs backends.
//!
//! ElevenLabs biases Scribe v2 towards a list of `keyterms`. The two endpoints
//! accept the same concept under different budgets and different transports,
//! so everything endpoint-independent lives here as a pure function.
//!
//! The rules below are the API's, not ours, and every one of them rejects the
//! **whole request** rather than the offending term. A dropped term degrades
//! one dictation; a 400 loses it entirely — so `sanitize` drops silently.

/// Longest term each endpoint accepts, inclusive.
///
/// Batch is 49 rather than 50 on purpose: the API documents the limit as
/// "must be *less than* 50 characters", and a 50-character term is what a
/// naive reading of "max 50" would send.
pub const BATCH_MAX_CHARS: usize = 49;
/// Realtime documents "a maximum length of 20 characters" — inclusive.
pub const REALTIME_MAX_CHARS: usize = 20;

/// How many terms each endpoint takes.
///
/// Batch's documented ceiling is 1000, but ElevenLabs applies a **20-second
/// minimum billable duration** to any request carrying more than 100 keyterms.
/// Beamer's utterances are seconds long, so going above 100 would multiply the
/// bill for a benefit no dictation-length clip can use. Deliberate cap — don't
/// "fix" it upward without pricing it first.
pub const BATCH_MAX_TERMS: usize = 100;
/// Realtime's hard ceiling.
pub const REALTIME_MAX_TERMS: usize = 50;

/// A keyterm may contain at most this many words after normalisation.
const MAX_WORDS: usize = 5;

/// Characters the API explicitly does not support inside a keyterm.
const FORBIDDEN: [char; 7] = ['<', '>', '{', '}', '[', ']', '\\'];

/// Trim, drop the terms the API would reject, de-duplicate, and cap the count.
///
/// Order is preserved so the vocabulary list's own ordering decides which terms
/// survive the cap — the user put the important ones where they put them.
pub fn sanitize(terms: &[String], max_terms: usize, max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for term in terms {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        // Character count, not byte length: a 20-character term of accented
        // or CJK text is well within the limit the API is counting.
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

/// Percent-encode one keyterm for use as a query-string value.
///
/// Keyterms legitimately contain spaces ("Deploy Purple"), and the realtime
/// endpoint takes them as repeated query parameters, so they cannot be
/// interpolated raw. Everything outside the RFC 3986 unreserved set is escaped
/// — deliberately conservative, since over-escaping is decoded back to the
/// same string while under-escaping corrupts the term or the URL.
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

    /// The probe that found the batch bug used a 60-character term: the API
    /// answers "All keywords must be less than 50 characters" and fails the
    /// whole request, so an over-long term must never reach it.
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

    /// The limit the API counts is characters, so a term of multi-byte
    /// characters must not be judged by its byte length — `"é".repeat(20)` is
    /// 40 bytes but 20 characters, and is perfectly legal at realtime's limit.
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

    /// Two vocabulary entries differing only in surrounding whitespace collapse
    /// to one term after trimming; sending it twice wastes part of the budget.
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

    /// The cap counts terms that survived, not terms that were offered — a
    /// long run of rejects at the front must not eat into the budget.
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

    /// A space in a keyterm is legal and common; left raw it would break the
    /// WebSocket upgrade URL outright.
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
