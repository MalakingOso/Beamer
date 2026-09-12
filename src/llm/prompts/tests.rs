//! Tests for [`super`].
//!
//! Split into their own file so `prompts.rs` keeps room under the project's
//! 500-line limit for the prompts themselves, which are the part that has to be
//! read while working. Nothing changed in the move — the same suite, dedented
//! one level.
//!
//! ⚠️ Same module rule as the parent: **no crate-rooted paths here.**
//! `src/bin/task_eval.rs` `#[path]`-includes `llm/mod.rs`, and there is no
//! `src/lib.rs` for a `crate::` path to resolve against.

use super::*;


/// Every category the policy suppresses, in the spec's own words. Written
/// out longhand: the point is to fail if one is dropped during an edit,
/// and a test that iterated over the prompt's own bullets would not.
/// The prompt for a fixed day, so the shape assertions below read one string
/// and not a moving target. Every one of them is about text that does not
/// depend on the date.
fn extract_prompt() -> String {
    extract_system(chrono::NaiveDate::from_ymd_opt(2026, 8, 23).unwrap())
}

#[test]
fn the_extraction_prompt_names_every_negative_category() {
    // v5a gates on an explicit "category" enum (see `scan` in the prompt)
    // rather than the bullet-list headers v2 used — naming each value here is
    // what suppresses it, same as before.
    for category in [
        "past",
        "someone_else",
        "hypothetical",
        "opinion",
        "fact",
        "aspiration",
        "rhetorical",
    ] {
        assert!(
            extract_prompt().contains(&format!("\"{category}\"")),
            "naming a category is what suppresses it; `{category}` is missing"
        );
    }
}

#[test]
fn the_extraction_prompt_gates_tasks_on_speaker_and_category_together() {
    // The compliance gap `HANDOFF.md` documents (a clause's own scan row
    // saying "someone_else" and it still leaking into `tasks` anyway) is a
    // model-behavior problem this rule can't fully close by itself — but the
    // rule still has to be stated, or there is nothing pushing back on it.
    let prompt = extract_prompt();
    assert!(prompt.contains(r#""subject_is_speaker": true AND "category": "task""#));
}

#[test]
fn the_extraction_prompt_carries_at_least_two_hard_negative_exemplars() {
    // Positive-only exemplars teach the model that output is always
    // expected, which is the same over-triggering failure the negative
    // categories exist to prevent. v5a's examples are `"scan": [...], "tasks": []`
    // rather than v2's bare `{"tasks": []}`, so the brace is dropped from the
    // match.
    let empty_answers = extract_prompt().matches(r#""tasks": []"#).count();
    assert!(
        empty_answers >= 2,
        "expected at least two exemplars answering with an empty task list, found {empty_answers}"
    );
}

#[test]
fn the_extraction_prompt_demands_verbatim_evidence() {
    // The parser rejects evidence that is not in the note. If the prompt
    // stopped asking for a verbatim span, every proposal would start
    // failing that check and extraction would silently return nothing.
    assert!(extract_prompt().contains("copied verbatim from the note"));
}

#[test]
fn the_extraction_prompt_tells_the_model_not_to_tidy_up_the_quote() {
    // "Verbatim" alone was not enough in practice: a model will still
    // silently capitalize a name or drop a filler word while "quoting" it,
    // which is exactly the kind of edit that fails the grounding check.
    // This bullet names the failure directly instead of trusting "verbatim"
    // to rule it out on its own. v5a's worked examples are all filler-free
    // (unlike v2's dedicated "um so i guess..." exemplar), so only the
    // instruction text is pinned here, not a worked demonstration of it.
    let prompt = extract_prompt();
    assert!(prompt.contains("copy-paste, not a transcription"));
    assert!(prompt.contains(r#"Preserve filler words ("um", "uh")"#));
}

#[test]
fn the_prompt_states_the_day_by_name_as_well_as_by_number() {
    // "Before Friday" is not resolvable from an ISO date alone, and asking a
    // language model to compute a weekday is asking it to be wrong.
    let prompt = extract_system(chrono::NaiveDate::from_ymd_opt(2026, 8, 23).unwrap());
    assert!(prompt.contains("Sunday"), "the weekday must be stated: {prompt}");
    assert!(prompt.contains("2026-08-23"));
    assert!(
        !prompt.contains("{TODAY}"),
        "an unfilled placeholder would reach the model as literal text"
    );
}

#[test]
fn the_prompt_moves_with_the_day_it_is_given() {
    let a = extract_system(chrono::NaiveDate::from_ymd_opt(2026, 8, 23).unwrap());
    let b = extract_system(chrono::NaiveDate::from_ymd_opt(2026, 12, 25).unwrap());
    assert_ne!(a, b);
    assert!(b.contains("2026-12-25") && b.contains("Friday"));
}

#[test]
fn the_prompt_forbids_guessing_a_date_and_still_asks_for_the_phrase() {
    // The whole precision argument for dates rests on these two sentences: a
    // model asked for a date will produce one, and the phrase is what makes an
    // unresolvable date fixable in one click instead of lost.
    let prompt = extract_prompt();
    assert!(prompt.contains("NEVER guess a date"));
    assert!(prompt.contains("sometime next week"), "the vague phrasings are named");
    assert!(prompt.contains(r#"Give it even when "due" is null"#));
}

#[test]
fn the_prompt_reserves_event_for_an_appointment_with_a_time() {
    assert!(extract_prompt()
        .contains(r#""kind" is "event" only for an appointment at a stated time"#));
}
