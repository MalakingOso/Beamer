//! Note body ↔ renderable blocks. Pure functions only, no I/O.
//!
//! Attachments live in the body as `[[beamer:<id>]]` tokens, one per line.
//! ⚠️ Tokens must never reach either model: strip before a call, restore after.
//! A token counts only alone on its line; anything else is literal text, and an
//! unmatched id renders as plain text so desync fails visibly.

/// One piece of a note body, in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block<'a> {
    /// A run of body lines, newlines between them preserved. A middle run
    /// carries no trailing newline (it is the separator before the token);
    /// the final run keeps whatever the body ends with, including trailing
    /// newlines, so a textarea bound to it round-trips an Enter at the end.
    /// May be empty: `parse` synthesizes empty runs so Text and Attachment
    /// strictly alternate, which is what gives the UI a textarea above and
    /// below every attachment.
    Text(&'a str),
    /// The id inside a token.
    Attachment(&'a str),
}

const OPEN: &str = "[[beamer:";
const CLOSE: &str = "]]";

/// Render the token for an id. The one place its spelling is written down.
pub fn token_for(id: &str) -> String {
    format!("{OPEN}{id}{CLOSE}")
}

/// The id in `line`, if the whole line is a token (surrounding whitespace tolerated).
fn token_id(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix(OPEN)?.strip_suffix(CLOSE)?;
    let ok = !inner.is_empty()
        && !inner.contains(char::is_whitespace)
        && !inner.contains('[')
        && !inner.contains(']');
    ok.then_some(inner)
}

/// `raw` slices concatenate back to the body exactly.
#[derive(Debug, Clone, Copy)]
struct Segment<'a> {
    raw: &'a str,
    /// `Some(id)` for a token line, `None` for a text run.
    id: Option<&'a str>,
}

/// Tile `body` into strictly alternating text/attachment segments.
fn segments(body: &str) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment> = Vec::new();
    let mut run_start = 0usize;
    let mut offset = 0usize;

    for piece in body.split_inclusive('\n') {
        let start = offset;
        offset += piece.len();
        let line_end = start + piece.strip_suffix('\n').map_or(piece.len(), str::len);

        if let Some(id) = token_id(&body[start..line_end]) {
            out.push(Segment { raw: &body[run_start..start], id: None });
            out.push(Segment { raw: piece, id: Some(id) });
            run_start = offset;
        }
    }
    out.push(Segment { raw: &body[run_start..], id: None });
    out
}

/// A middle text segment's editable content: its raw slice without the one
/// trailing newline that separates it from the token that follows. The final
/// segment has no such separator, so its raw slice is content as-is.
fn content(raw: &str) -> &str {
    raw.strip_suffix('\n').unwrap_or(raw)
}

/// Split a body into the blocks that render it. A token-free body yields exactly one `Text`.
pub fn parse(body: &str) -> Vec<Block<'_>> {
    let segs = segments(body);
    let last = segs.len().saturating_sub(1);
    segs.into_iter()
        .enumerate()
        .map(|(i, s)| match s.id {
            Some(id) => Block::Attachment(id),
            // Only a middle run has a separator newline to remove. Stripping
            // the final run ate an Enter at the end of the note on every
            // re-render: the store held "hello\n" but the textarea was given
            // "hello", so the controlled value snapped back and the cursor
            // jumped to the previous line.
            None if i == last => Block::Text(s.raw),
            None => Block::Text(content(s.raw)),
        })
        .collect()
}

/// Just the text runs, in order — one cleanup call each.
pub fn text_runs(body: &str) -> Vec<&str> {
    let segs = segments(body);
    let last = segs.len().saturating_sub(1);
    segs.into_iter()
        .enumerate()
        .filter(|(_, s)| s.id.is_none())
        .map(|(i, s)| if i == last { s.raw } else { content(s.raw) })
        .collect()
}

/// The body with every token line removed (model input; what `search` matches).
pub fn plain_text(body: &str) -> String {
    segments(body)
        .into_iter()
        .filter(|s| s.id.is_none())
        .map(|s| s.raw)
        .collect()
}

/// Ids referenced by the body, in order. Duplicates kept for `prune_attachments`.
pub fn referenced_ids(body: &str) -> Vec<&str> {
    segments(body).into_iter().filter_map(|s| s.id).collect()
}

/// Put cleaned runs back, tokens untouched. `None`/blank keeps the original;
/// a blank original is never substituted (an answer for one is a misaligned index).
pub fn reassemble(body: &str, cleaned_runs: &[Option<String>]) -> String {
    let segs = segments(body);
    let last = segs.len().saturating_sub(1);
    let mut out = String::with_capacity(body.len());
    let mut run = 0usize;

    for (i, seg) in segs.iter().enumerate() {
        if seg.id.is_some() {
            out.push_str(seg.raw);
            continue;
        }
        let original = content(seg.raw);
        let replacement = cleaned_runs
            .get(run)
            .and_then(Option::as_ref)
            .filter(|s| !s.trim().is_empty())
            .filter(|_| !original.trim().is_empty());
        run += 1;

        match replacement {
            Some(text) => {
                out.push_str(text);
                // Keep the token that follows alone on its line.
                if i != last {
                    out.push('\n');
                }
            }
            None => out.push_str(seg.raw),
        }
    }
    out
}

/// Replace the `index`th text run. Out-of-range is a no-op (render-time index can go stale).
pub fn set_run(body: &str, index: usize, text: &str) -> String {
    let segs = segments(body);
    let last = segs.len().saturating_sub(1);
    let mut out = String::with_capacity(body.len() + text.len());
    let mut run = 0usize;

    for (i, seg) in segs.iter().enumerate() {
        if seg.id.is_some() {
            out.push_str(seg.raw);
            continue;
        }
        if run == index {
            out.push_str(text);
            // An empty run needs no separator: the following token already starts a line.
            if i != last && !text.is_empty() {
                out.push('\n');
            }
        } else {
            out.push_str(seg.raw);
        }
        run += 1;
    }
    out
}

/// Add a token for `id` on its own line, at the end of the body or the start.
pub fn insert_token(body: &str, id: &str, at_end: bool) -> String {
    let token = token_for(id);
    if body.is_empty() {
        return token;
    }
    if at_end {
        let sep = if body.ends_with('\n') { "" } else { "\n" };
        format!("{body}{sep}{token}")
    } else {
        format!("{token}\n{body}")
    }
}

/// Remove every token line for `id`, closing the gap.
pub fn remove_token(body: &str, id: &str) -> String {
    let segs = segments(body);
    let mut out = String::with_capacity(body.len());
    for seg in &segs {
        if seg.id == Some(id) {
            continue;
        }
        out.push_str(seg.raw);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG: &str = "[[beamer:img1]]";

    #[test]
    fn a_body_with_no_tokens_is_exactly_one_run() {
        let body = "call the vet\nabout Biscuit";
        assert_eq!(parse(body), vec![Block::Text(body)]);
        assert_eq!(text_runs(body), vec![body]);
    }

    #[test]
    fn an_empty_body_still_yields_one_empty_run() {
        assert_eq!(parse(""), vec![Block::Text("")]);
    }

    #[test]
    fn two_images_yield_three_text_runs_and_no_run_holds_a_token() {
        let body = format!("above\n{IMG}\nmiddle\n[[beamer:img2]]\nbelow");
        let runs = text_runs(&body);
        assert_eq!(runs, vec!["above", "middle", "below"]);
        for run in &runs {
            assert!(
                !run.contains("[[beamer:"),
                "a token reaching s1-mini comes back as garbage at HTTP 200: {run:?}"
            );
        }
    }

    #[test]
    fn text_and_attachment_blocks_strictly_alternate() {
        // Empty runs give the UI a textarea above/below every attachment.
        let body = format!("{IMG}\n[[beamer:img2]]");
        assert_eq!(
            parse(&body),
            vec![
                Block::Text(""),
                Block::Attachment("img1"),
                Block::Text(""),
                Block::Attachment("img2"),
                Block::Text(""),
            ]
        );
    }

    #[test]
    fn a_token_mid_line_stays_literal_text() {
        let body = format!("see {IMG} there");
        assert_eq!(parse(&body), vec![Block::Text(body.as_str())]);
        assert!(referenced_ids(&body).is_empty());
    }

    #[test]
    fn a_malformed_token_stays_literal_text() {
        for line in ["[[beamer:]]", "[[beamer:a b]]", "[[beamer:x]", "[[other:x]]"] {
            assert_eq!(parse(line), vec![Block::Text(line)], "on {line:?}");
        }
    }

    #[test]
    fn surrounding_whitespace_on_a_token_line_is_tolerated() {
        let body = format!("a\n  {IMG}  \nb");
        assert_eq!(referenced_ids(&body), vec!["img1"]);
    }

    #[test]
    fn plain_text_removes_exactly_the_token_lines() {
        let body = format!("above\n{IMG}\nbelow");
        assert_eq!(plain_text(&body), "above\nbelow");
        assert!(!plain_text(&body).contains("beamer"));
    }

    #[test]
    fn plain_text_leaves_a_token_free_body_alone() {
        let body = "nothing to strip\nhere";
        assert_eq!(plain_text(body), body);
    }

    #[test]
    fn parse_and_reassemble_round_trip_untouched() {
        for body in [
            "",
            "plain",
            "trailing newline\n",
            &format!("above\n{IMG}\nbelow"),
            &format!("{IMG}\nbelow"),
            &format!("above\n{IMG}"),
            IMG,
            &format!("a\n\n{IMG}\n\nb\n"),
        ] {
            let runs: Vec<Option<String>> = text_runs(body).iter().map(|_| None).collect();
            assert_eq!(
                reassemble(body, &runs),
                *body,
                "an all-None result must leave the body byte-identical: {body:?}"
            );
        }
    }

    #[test]
    fn reassemble_keeps_the_attachment_between_the_runs_it_cleaned() {
        let body = format!("um so ring sarah\n{IMG}\nand the deck");
        let cleaned = vec![
            Some("Ring Sarah.".to_string()),
            Some("And the deck.".to_string()),
        ];
        let got = reassemble(&body, &cleaned);
        assert_eq!(got, format!("Ring Sarah.\n{IMG}\nAnd the deck."));
        assert_eq!(
            referenced_ids(&got),
            vec!["img1"],
            "the token must still be alone on its line, or it becomes literal text"
        );
    }

    #[test]
    fn an_empty_reply_for_one_run_leaves_that_run_intact() {
        let body = format!("keep me\n{IMG}\nclean me");
        let got = reassemble(&body, &[Some(String::new()), Some("Clean me.".into())]);
        assert_eq!(got, format!("keep me\n{IMG}\nClean me."));
    }

    #[test]
    fn a_blank_run_is_never_substituted() {
        // Blank runs are never sent, so an answer for one is a misaligned index.
        let body = format!("{IMG}\ntext");
        let got = reassemble(&body, &[Some("invented".into()), Some("Text.".into())]);
        assert_eq!(got, format!("{IMG}\nText."));
    }

    #[test]
    fn a_short_runs_slice_leaves_the_rest_alone() {
        let body = format!("one\n{IMG}\ntwo");
        assert_eq!(reassemble(&body, &[]), body);
    }

    #[test]
    fn set_run_edits_one_textarea_without_moving_the_others() {
        let body = format!("above\n{IMG}\nbelow");
        assert_eq!(set_run(&body, 0, "ABOVE"), format!("ABOVE\n{IMG}\nbelow"));
        assert_eq!(set_run(&body, 1, "BELOW"), format!("above\n{IMG}\nBELOW"));
        assert_eq!(set_run(&body, 9, "nope"), body, "an out-of-range index is a no-op");
    }

    #[test]
    fn clearing_the_run_above_a_leading_image_leaves_no_blank_line() {
        let body = format!("above\n{IMG}\nbelow");
        let got = set_run(&body, 0, "");
        assert_eq!(got, format!("{IMG}\nbelow"));
        assert_eq!(referenced_ids(&got), vec!["img1"]);
    }

    #[test]
    fn set_run_accepts_multi_line_text() {
        let body = format!("a\n{IMG}");
        assert_eq!(set_run(&body, 0, "a\nb\nc"), format!("a\nb\nc\n{IMG}"));
    }

    #[test]
    fn a_trailing_newline_survives_parse_so_an_enter_at_the_end_sticks() {
        // The textarea is controlled: the store holds what `oninput` wrote,
        // then `parse` decides what the textarea is given back. Stripping the
        // final run's trailing newline here snapped the value back and moved
        // the cursor up a line on every Enter at the end of the note.
        assert_eq!(parse("hello\n"), vec![Block::Text("hello\n")]);
        assert_eq!(parse("hello\n\n"), vec![Block::Text("hello\n\n")]);
        assert_eq!(text_runs("hello\n"), vec!["hello\n"]);
        // An Enter in the middle always round-tripped, which is why only a
        // trailing Enter vanished.
        assert_eq!(parse("hello\nworld"), vec![Block::Text("hello\nworld")]);
    }

    #[test]
    fn a_middle_run_still_drops_its_separator_newline() {
        let body = format!("above\n{IMG}\nbelow");
        assert_eq!(
            parse(&body),
            vec![
                Block::Text("above"),
                Block::Attachment("img1"),
                Block::Text("below"),
            ]
        );
        assert_eq!(text_runs(&body), vec!["above", "below"]);
    }

    #[test]
    fn set_run_then_parse_round_trips_a_trailing_enter() {
        let body = set_run("hello", 0, "hello\n");
        assert_eq!(body, "hello\n");
        assert_eq!(parse(&body), vec![Block::Text("hello\n")]);
        let body = set_run(&format!("above\n{IMG}\nbelow"), 1, "below\n");
        assert_eq!(text_runs(&body), vec!["above", "below\n"]);
    }

    #[test]
    fn insert_token_puts_it_on_its_own_line() {
        assert_eq!(insert_token("", "x", true), "[[beamer:x]]");
        assert_eq!(insert_token("note", "x", true), "note\n[[beamer:x]]");
        assert_eq!(insert_token("note\n", "x", true), "note\n[[beamer:x]]");
        assert_eq!(insert_token("note", "x", false), "[[beamer:x]]\nnote");
        assert_eq!(referenced_ids(&insert_token("note", "x", true)), vec!["x"]);
    }

    #[test]
    fn remove_token_closes_the_gap_between_the_runs() {
        let body = format!("above\n{IMG}\nbelow");
        assert_eq!(remove_token(&body, "img1"), "above\nbelow");
        assert_eq!(remove_token(&body, "other"), body, "an unknown id changes nothing");
    }

    #[test]
    fn remove_token_drops_every_copy_of_the_id() {
        let body = format!("{IMG}\nmid\n{IMG}\nend");
        assert_eq!(remove_token(&body, "img1"), "mid\nend");
    }

    #[test]
    fn referenced_ids_reads_in_order_and_keeps_duplicates() {
        let body = format!("{IMG}\n[[beamer:b]]\n{IMG}");
        assert_eq!(referenced_ids(&body), vec!["img1", "b", "img1"]);
    }

    #[test]
    fn the_token_opener_is_the_literal_the_chat_guard_duplicates() {
        // `src/llm/chat.rs` cannot import this module, so it duplicates the literal.
        assert_eq!(OPEN, "[[beamer:");
        assert!(token_for("x").starts_with("[[beamer:"));
    }

    #[test]
    fn token_for_matches_what_parse_accepts() {
        let token = token_for("abc-0001");
        assert_eq!(referenced_ids(&token), vec!["abc-0001"]);
    }
}
