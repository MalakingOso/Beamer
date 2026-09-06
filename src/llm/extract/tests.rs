//! Tests for [`super`].
//!
//! Split into their own file so `extract.rs` keeps room under the project's
//! 500-line limit for the validation gates, which are the part that has to be
//! read while working.
//!
//! ⚠️ Same module rule as the parent: **no crate-rooted paths here.**

use super::*;


const NOTE: &str =
    "I need to call the vet about Milo tomorrow. Sarah is sending the invoice on Tuesday.";

/// The day the dated tests resolve against. Fixed, so nothing here depends on
/// when the suite is run.
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, 23).unwrap()
}

fn one_task(evidence: &str, confidence: f32) -> String {
    format!(
        r#"{{"tasks":[{{"text":"Call the vet","evidence":"{evidence}","confidence":{confidence}}}]}}"#
    )
}

/// One task with a date, so each gate can be exercised on its own.
fn dated(due: &str, phrase: &str, kind: &str) -> String {
    let due = if due == "null" { "null".to_string() } else { format!("\"{due}\"") };
    format!(
        r#"{{"tasks":[{{"text":"Call the vet","evidence":"I need to call the vet about Milo tomorrow","confidence":0.9,"due":{due},"due_all_day":true,"due_phrase":"{phrase}","kind":"{kind}"}}]}}"#
    )
}

/// Parse against the fixed test day.
fn parse(body: &str, note: &str, floor: f32) -> Result<Vec<ProposedTask>, ChatError> {
    parse_tasks(body, note, floor, today())
}

#[test]
fn evidence_not_in_note_is_rejected_as_fabrication() {
    let body = one_task("I promised to rewire the whole house", 0.99);
    assert!(
        parse(&body, NOTE, 0.5).unwrap().is_empty(),
        "a task whose span is not in the note is invented, and confidence \
         says nothing about that — a fabricating model is confident"
    );
}

#[test]
fn empty_evidence_is_not_grounding() {
    // Every string contains the empty string. Without an explicit guard a
    // model that simply omitted `evidence` would pass the one check that
    // does not depend on its judgment.
    let body = one_task("", 0.99);
    assert!(parse(&body, NOTE, 0.5).unwrap().is_empty());
}

#[test]
fn an_empty_task_list_is_a_successful_answer_not_an_error() {
    // The common case. Most notes contain no tasks, and treating that as a
    // failure would light up a retry affordance on every ordinary note.
    assert_eq!(parse(r#"{"tasks":[]}"#, NOTE, 0.5).unwrap(), vec![]);
    assert_eq!(parse(r#"{}"#, NOTE, 0.5).unwrap(), vec![]);
}

#[test]
fn a_leaked_reasoning_block_is_stripped_before_the_json() {
    let inner = one_task("I need to call the vet about Milo tomorrow", 0.9);
    for wrapped in [
        format!("we need to check the note for tasks...</think>\n{inner}"),
        format!("reasoning here</ifm|think_fast>\n{inner}"),
        format!("more reasoning</ifm|think_faster>{inner}"),
        format!("<THINK>reasoning</THINK>\n{inner}"),
    ] {
        let got = parse(&wrapped, NOTE, 0.5).unwrap();
        assert_eq!(got.len(), 1, "failed on: {wrapped}");
    }
}

#[test]
fn a_body_with_no_think_tag_is_left_alone() {
    // The common case, and the one that must cost nothing: a server that
    // already split reasoning out sends `content` with no tag in it at all.
    assert_eq!(strip_think_tags("plain text, no tags"), "plain text, no tags");
}

#[test]
fn only_the_last_closing_think_tag_is_honoured() {
    // A response can legitimately contain the word "think" more than once
    // before the real answer starts; stripping at the first match would cut
    // into the model's own reasoning instead of past all of it.
    let body = "first pass</think>more thinking</think>\n{\"tasks\":[]}";
    assert_eq!(strip_think_tags(body), "{\"tasks\":[]}");
}

#[test]
fn fenced_json_is_tolerated() {
    let inner = one_task("I need to call the vet about Milo tomorrow", 0.9);
    for wrapped in [
        format!("```json\n{inner}\n```"),
        format!("```\n{inner}\n```"),
        format!("  ```json\n{inner}\n```  "),
    ] {
        let got = parse(&wrapped, NOTE, 0.5).unwrap();
        assert_eq!(got.len(), 1, "failed on: {wrapped}");
    }
}

#[test]
fn a_task_at_exactly_the_floor_is_kept() {
    let body = one_task("I need to call the vet about Milo tomorrow", 0.5);
    assert_eq!(parse(&body, NOTE, 0.5).unwrap().len(), 1);

    let below = one_task("I need to call the vet about Milo tomorrow", 0.49);
    assert!(parse(&below, NOTE, 0.5).unwrap().is_empty());
}

#[test]
fn the_floor_is_applied_here_and_not_left_to_the_ui() {
    // A row nobody is ever shown is not a labelled example. Letting it
    // through to be filtered at render time would put a decision nobody
    // made into tasks.json and poison the eval corpus.
    let body = one_task("I need to call the vet about Milo tomorrow", 0.2);
    assert!(parse(&body, NOTE, 0.5).unwrap().is_empty());
}

#[test]
fn grounding_survives_the_whitespace_a_transcript_carries() {
    let note = "I need to  call the vet\nabout Milo tomorrow.";
    let body = one_task("I need to call the vet about milo tomorrow", 0.9);
    assert_eq!(
        parse(&body, note, 0.5).unwrap().len(),
        1,
        "a doubled space or a line break in the transcript must not read as a fabrication"
    );
}

#[test]
fn an_out_of_range_confidence_is_clamped_not_fatal() {
    let body = one_task("I need to call the vet about Milo tomorrow", 4.2);
    let got = parse(&body, NOTE, 0.5).unwrap();
    assert_eq!(got[0].confidence, 1.0);
}

#[test]
fn malformed_json_is_an_error_not_an_empty_list() {
    // These must be distinguishable: an empty list marks the stage Done,
    // a parse failure marks it Failed and offers a retry.
    assert!(matches!(
        parse("the model said something else entirely", NOTE, 0.5),
        Err(ChatError::Malformed(_))
    ));
}

#[test]
fn the_extraction_request_asks_the_server_to_constrain_the_grammar() {
    let req = build_request(&ExtractConfig::default(), NOTE, today());
    assert_eq!(req.model, "gemma-4-E4B_q4_0-it");
    assert!(req.response_format.is_some());
    assert_eq!(req.messages[0].content, prompts::extract_system(today()));
assert!(
    req.messages[0].content.contains("2026-08-23"),
    "the model cannot resolve a relative date it was never told the day for"
);
    assert_eq!(req.messages[1].content, NOTE, "the note is sent as-is");
}

#[test]
fn a_resolvable_date_survives_with_its_phrase() {
    let got = parse(&dated("2026-08-28", "tomorrow", "todo"), NOTE, 0.5).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].due.as_deref(), Some("2026-08-28"));
    assert!(got[0].due_all_day);
    assert_eq!(got[0].due_phrase.as_deref(), Some("tomorrow"));
    assert_eq!(got[0].kind, TaskKind::Todo);
}

#[test]
fn an_ungrounded_due_phrase_drops_both_the_phrase_and_the_date() {
    // The strongest guard available against an invented date, and it costs
    // nothing new: a model that made the date up made the phrase up too.
    let got = parse(&dated("2026-08-28", "before the AGM in Zurich", "todo"), NOTE, 0.5)
        .unwrap();
    assert_eq!(got.len(), 1, "the task itself must survive a date failure");
    assert_eq!(got[0].due, None);
    assert_eq!(got[0].due_phrase, None);
}

#[test]
fn an_unparseable_due_keeps_the_phrase_and_drops_the_date() {
    let got = parse(&dated("next Friday-ish", "tomorrow", "todo"), NOTE, 0.5).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].due, None);
    assert_eq!(
        got[0].due_phrase.as_deref(),
        Some("tomorrow"),
        "the phrase is what lets the UI offer a picker instead of a shrug"
    );
}

#[test]
fn a_due_date_in_the_past_drops_to_phrase_only() {
    // The classic silent failure: "Friday" resolved against the wrong year.
    // The task looks perfect and is filed two years ago.
    let got = parse(&dated("2024-08-28", "tomorrow", "todo"), NOTE, 0.5).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].due, None);
    assert_eq!(got[0].due_phrase.as_deref(), Some("tomorrow"));
}

#[test]
fn a_due_date_centuries_out_drops_to_phrase_only() {
    // The mirror failure: a hallucinated year passes every other gate and
    // would otherwise be exported straight to the calendar.
    let got = parse(&dated("2099-01-01", "tomorrow", "todo"), NOTE, 0.5).unwrap();
    assert_eq!(got.len(), 1, "the task itself must survive a date failure");
    assert_eq!(got[0].due, None);
    assert_eq!(got[0].due_phrase.as_deref(), Some("tomorrow"));
}

#[test]
fn yesterday_is_inside_the_slack_but_last_week_is_not() {
    // One day of slack, not zero, so a pass that runs just after midnight on
    // something due "today" is not thrown away.
    let ok = parse(&dated("2026-08-22", "tomorrow", "todo"), NOTE, 0.5).unwrap();
    assert_eq!(ok[0].due.as_deref(), Some("2026-08-22"));

    let stale = parse(&dated("2026-08-16", "tomorrow", "todo"), NOTE, 0.5).unwrap();
    assert_eq!(stale[0].due, None);
}

#[test]
fn an_unknown_kind_becomes_a_todo() {
    for kind in ["reminder", "EVENT ", "", "Todo"] {
        let got = parse(&dated("2026-08-28", "tomorrow", kind), NOTE, 0.5).unwrap();
        assert_eq!(got.len(), 1, "on {kind:?}");
        let expected = if kind.trim().eq_ignore_ascii_case("event") {
            TaskKind::Event
        } else {
            TaskKind::Todo
        };
        assert_eq!(got[0].kind, expected, "on {kind:?}");
    }
}

#[test]
fn an_appointment_with_a_time_keeps_its_time_and_is_an_event() {
    let body = r#"{"tasks":[{"text":"Standup","evidence":"Sarah is sending the invoice on Tuesday","confidence":0.9,"due":"2026-08-25T09:00:00","due_all_day":false,"due_phrase":"on Tuesday","kind":"event"}]}"#;
    let got = parse(body, NOTE, 0.5).unwrap();
    assert_eq!(got[0].due.as_deref(), Some("2026-08-25T09:00:00"));
    assert!(!got[0].due_all_day);
    assert_eq!(got[0].kind, TaskKind::Event);
}

#[test]
fn a_task_with_no_date_at_all_is_undated_and_still_a_task() {
    // The common case, and the one that must not regress: most notes name no
    // time at all, and the pre-dating shape of the response is still valid.
    let got = parse(&one_task("I need to call the vet about Milo tomorrow", 0.9), NOTE, 0.5)
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].due, None);
    assert_eq!(got[0].due_phrase, None);
    assert_eq!(got[0].kind, TaskKind::Todo);
}

#[test]
fn a_date_failure_never_costs_the_task() {
    // Stated once, over every gate: this is the trade the whole date design
    // rests on. A wrong date is recoverable in one click; a lost commitment is
    // not recoverable at all.
    for body in [
        dated("2026-08-28", "not in the note at all", "todo"),
        dated("gibberish", "tomorrow", "todo"),
        dated("1999-01-01", "tomorrow", "todo"),
        dated("2026-08-28", "tomorrow", "appointment"),
    ] {
        let got = parse(&body, NOTE, 0.5).unwrap();
        assert_eq!(got.len(), 1, "the task vanished on: {body}");
        assert_eq!(got[0].text, "Call the vet");
    }
}

#[test]
fn a_due_phrase_grounds_through_the_whitespace_a_transcript_carries() {
    let note = "I need to  call the vet\nabout Milo tomorrow.";
    let got = parse(&dated("2026-08-24", "About Milo  tomorrow", "todo"), note, 0.5).unwrap();
    assert_eq!(
        got[0].due_phrase.as_deref(),
        Some("About Milo  tomorrow"),
        "a doubled space in the transcript must not read as an invented date"
    );
}
