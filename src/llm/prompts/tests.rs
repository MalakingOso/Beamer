//! Tests for [`super`]: they pin the prompt phrases the parser and the
//! precision policy depend on. A separate file keeps `prompts.rs` under the
//! 500-line limit. Same rule as the parent: no crate-rooted paths here.

use super::*;


/// The prompt for a fixed day, so the shape assertions below read one string.
/// None of them depend on the date.
fn extract_prompt() -> String {
    extract_system(chrono::NaiveDate::from_ymd_opt(2026, 8, 23).unwrap())
}

#[test]
fn the_extraction_prompt_names_every_negative_category() {
    // Naming a category value is what suppresses it. Written out longhand so
    // dropping one in an edit fails; iterating the prompt's own list would not.
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
fn the_extraction_prompt_carries_at_least_two_hard_negative_exemplars() {
    // Positive-only exemplars teach the model that output is always expected,
    // the same over-triggering the negative categories exist to prevent. No
    // leading brace in the match: exemplars put `"scan"` before `"tasks"`.
    let empty_answers = extract_prompt().matches(r#""tasks": []"#).count();
    assert!(
        empty_answers >= 2,
        "expected at least two exemplars answering with an empty task list, found {empty_answers}"
    );
}

#[test]
fn the_extraction_prompt_tells_the_model_not_to_tidy_up_the_quote() {
    // "Verbatim" alone was not enough: a model still capitalizes a name or
    // drops a filler word while "quoting", which fails the grounding check.
    // The exemplars are filler-free, so only the instruction text is pinned.
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
fn the_prompt_forbids_guessing_a_date_and_still_asks_for_the_phrase() {
    // The whole precision argument for dates rests on these two sentences: a
    // model asked for a date will produce one, and the phrase is what makes an
    // unresolvable date fixable in one click instead of lost.
    let prompt = extract_prompt();
    assert!(prompt.contains("NEVER guess a date"));
    assert!(prompt.contains("sometime next week"), "the vague phrasings are named");
    assert!(prompt.contains(r#"Give it even when "due" is null"#));
}
