//! How a note body maps to renderable blocks.
//!
//! A note is one string. Attachments live in it as **placeholder tokens** —
//! `[[beamer:<id>]]` alone on a line — which keeps reading order in the body
//! itself rather than in a parallel structure that could drift from it.
//!
//! ⚠️ **A token must never reach either model.** `body` is handed to s1-mini and
//! overwritten wholesale with the reply, and s1-mini is a trained wire format,
//! not a chat model: out-of-distribution input comes back garbled at HTTP 200
//! with a plausible body (see the standing warning in `llm/prompts.rs`). So
//! tokens are *removed* before a call and restored after — never escaped, never
//! quoted. [`parse`] splits the body into runs and [`reassemble`] puts the
//! answers back at fixed token positions; that pair is the whole mechanism.
//!
//! Pure functions only. No Dioxus, no I/O — the same discipline that keeps
//! `ui::note_layout` testable.
//!
//! **The grammar is deliberately narrow.** A token counts only when it is alone
//! on its own line after trimming; anything else, including a token mid-line, is
//! literal text. `[[` cannot come out of dictation, so no transcript can
//! accidentally produce one, and an id with no matching `Attachment` renders as
//! plain text — a desynchronised note fails visibly rather than silently
//! swallowing a line.

/// One piece of a note body, in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block<'a> {
    /// A run of body lines, newlines between them preserved, no trailing one.
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

/// The id in `line`, if the whole line is a token.
///
/// Whitespace around it is tolerated — a textarea edit can leave a stray space
/// — but nothing else on the line is, and an id may not contain whitespace or
/// brackets of its own.
fn token_id(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix(OPEN)?.strip_suffix(CLOSE)?;
    let ok = !inner.is_empty()
        && !inner.contains(char::is_whitespace)
        && !inner.contains('[')
        && !inner.contains(']');
    ok.then_some(inner)
}

/// One tile of the body. `raw` slices concatenate back to the body exactly,
/// which is what lets `reassemble` be byte-identical when nothing changed.
#[derive(Debug, Clone, Copy)]
struct Segment<'a> {
    raw: &'a str,
    /// `Some(id)` for a token line, `None` for a run of text lines.
    id: Option<&'a str>,
}

/// Tile `body` into strictly alternating text and attachment segments,
/// starting and ending with a text segment (either may be empty).
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

/// A text segment's editable content: its raw slice without the one trailing
/// newline that separates it from what follows.
fn content(raw: &str) -> &str {
    raw.strip_suffix('\n').unwrap_or(raw)
}

/// Split a body into the blocks that render it.
///
/// A body with no tokens yields **exactly one** `Text` run holding the whole
/// body — today's single-textarea behaviour, reproduced by construction rather
/// than by a fast-path branch that could get out of step.
pub fn parse(body: &str) -> Vec<Block<'_>> {
    segments(body)
        .into_iter()
        .map(|s| match s.id {
            Some(id) => Block::Attachment(id),
            None => Block::Text(content(s.raw)),
        })
        .collect()
}

/// Just the text runs, in order. What the cleanup pass is given, one call each.
pub fn text_runs(body: &str) -> Vec<&str> {
    segments(body)
        .into_iter()
        .filter(|s| s.id.is_none())
        .map(|s| content(s.raw))
        .collect()
}

/// The body with every token line removed.
///
/// What reaches the extraction model, and what `NoteStore::search` matches over
/// — without it every attachment-bearing note would match the query "beamer"
/// via its own tokens.
pub fn plain_text(body: &str) -> String {
    segments(body)
        .into_iter()
        .filter(|s| s.id.is_none())
        .map(|s| s.raw)
        .collect()
}

/// Ids referenced by the body, in reading order. Duplicates are kept — a
/// duplicate is a fact about the body, and `prune_attachments` needs to see it.
pub fn referenced_ids(body: &str) -> Vec<&str> {
    segments(body).into_iter().filter_map(|s| s.id).collect()
}

/// Put cleaned text runs back, leaving every token exactly where it was.
///
/// `cleaned_runs` is one entry per `Text` run, in `parse` order. `None` — and
/// an empty or whitespace-only `Some` — keeps the original run. So an
/// all-`None` result returns `body` byte for byte, which is what makes
/// `apply_cleanup`'s existing `!cleaned.trim().is_empty()` guard still
/// meaningful: **a run can never become empty through this function.**
///
/// A run that was itself blank is never substituted either. Blank runs are not
/// sent to the model (`pipeline` skips them), so an answer for one could only
/// come from a misaligned index — and writing text into the gap above an image
/// is exactly the corruption that would be hardest to notice.
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
                // A run followed by a token must still end the line, or the
                // token stops being alone on its own and becomes literal text.
                if i != last {
                    out.push('\n');
                }
            }
            None => out.push_str(seg.raw),
        }
    }
    out
}

/// Replace the `index`th text run with `text` — the UI's per-textarea write.
///
/// Out-of-range is a no-op returning the body unchanged, rather than a panic:
/// the caller is a render-time index and a note can be rewritten by a model
/// pass between render and keystroke.
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
            // An empty run needs no separator: the token that follows it
            // already starts a line, either at the body start or after the
            // previous token's own newline.
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
///
/// Only the end is reachable from the UI today; dropping *between* two runs is
/// deferred. The `at_end: false` arm exists because a note whose body is empty
/// but for one attachment is otherwise unreachable, and it is tested.
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
///
/// The text runs either side merge, which is what the user asked for by
/// deleting the block between them.
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
        // Today's behaviour, and the thing segmentation must not change: one
        // run means one cleanup call carrying the whole body, as before.
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
        // The UI depends on this: without a synthesized empty run there would
        // be no textarea above a leading image or below a trailing one.
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
        // `Cleaned::NothingToChange` arrives as an empty string. Substituting
        // it would blank half the note.
        let body = format!("keep me\n{IMG}\nclean me");
        let got = reassemble(&body, &[Some(String::new()), Some("Clean me.".into())]);
        assert_eq!(got, format!("keep me\n{IMG}\nClean me."));
    }

    #[test]
    fn a_blank_run_is_never_substituted() {
        // Blank runs are not sent, so an answer for one is a misaligned index.
        // Writing text into the gap above an image is the corruption that
        // would be hardest to spot afterwards.
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
    fn token_for_matches_what_parse_accepts() {
        // One spelling, in one place. A drift between writer and reader would
        // turn every new attachment into literal text.
        let token = token_for("abc-0001");
        assert_eq!(referenced_ids(&token), vec!["abc-0001"]);
    }
}
