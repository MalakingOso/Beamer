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


const ALL_STYLINGS: [Styling; 4] = [
    Styling::Casual,
    Styling::SemiCasual,
    Styling::SemiFormal,
    Styling::Formal,
];
const ALL_STRUCTURES: [Structure; 2] = [Structure::Prose, Structure::Lists];
const ALL_CONTEXTS: [NoteContext; 2] = [NoteContext::General, NoteContext::Email];

/// Pull the three values back out of an emitted control line, failing the
/// test if the shape is not exactly what the model expects.
fn split_values(line: &str) -> (String, String, String) {
    let inner = line
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or_else(|| panic!("control line is not bracketed: {line:?}"));
    let parts: Vec<&str> = inner.split("] [").collect();
    assert_eq!(parts.len(), 3, "control line must carry exactly three groups: {line:?}");
    let value = |part: &str, key: &str| -> String {
        part.strip_prefix(&format!("{key}: "))
            .unwrap_or_else(|| panic!("expected a {key} group, got {part:?}"))
            .to_string()
    };
    (
        value(parts[0], "Styling"),
        value(parts[1], "Structure"),
        value(parts[2], "Context"),
    )
}

#[test]
fn system_prompt_is_byte_identical_to_the_model_card() {
    // Spelled out longhand instead of being rebuilt the way the constant is:
    // a test that composed the string by the same route would happily agree
    // with a later reflow, which is precisely the change that must fail.
    let from_the_model_card = "You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.";
    assert_eq!(
        CLEANUP_SYSTEM, from_the_model_card,
        "s1-mini was trained on this exact sentence; rewording it degrades every \
         cleanup pass and the server still answers 200"
    );
}

#[test]
fn control_line_only_ever_emits_trained_values() {
    // The trained sets, written out here rather than read from `wire()` —
    // comparing the output against the function that produced it would pass
    // no matter what either of them said.
    let trained_stylings = ["casual", "semi-casual", "semi-formal", "formal"];
    let trained_structures = ["prose", "lists"];
    let trained_contexts = ["general", "email"];

    let mut seen = 0;
    for styling in ALL_STYLINGS {
        for structure in ALL_STRUCTURES {
            for context in ALL_CONTEXTS {
                let line = control_line(styling, structure, context);
                let (s, t, c) = split_values(&line);
                assert!(
                    trained_stylings.contains(&s.as_str()),
                    "{s:?} is outside the trained styling set; the model would \
                     hallucinate and still return 200"
                );
                assert!(
                    trained_structures.contains(&t.as_str()),
                    "{t:?} is outside the trained structure set"
                );
                assert!(
                    trained_contexts.contains(&c.as_str()),
                    "{c:?} is outside the trained context set"
                );
                seen += 1;
            }
        }
    }
    assert_eq!(seen, 16, "every combination of the three axes must be covered");
}

#[test]
fn the_default_axes_are_the_line_the_spec_prints() {
    assert_eq!(
        control_line(Styling::SemiFormal, Structure::Prose, NoteContext::General),
        "[Styling: semi-formal] [Structure: prose] [Context: general]"
    );
}

#[test]
fn defaults_match_the_configured_ones() {
    assert_eq!(Styling::default(), Styling::SemiFormal);
    assert_eq!(Structure::default(), Structure::Lists);
    assert_eq!(NoteContext::default(), NoteContext::General);
}

#[test]
fn an_unrecognized_config_value_falls_back_to_the_trained_default() {
    // Same rule as `NoteColor::from_config_name`: a typo in the TOML must
    // not reach the model, because an out-of-set value is not rejected —
    // it is answered with garbage.
    assert_eq!(Styling::from_config_name("baroque"), Styling::SemiFormal);
    assert_eq!(Structure::from_config_name("tables"), Structure::Lists);
    assert_eq!(NoteContext::from_config_name("sms"), NoteContext::General);
    assert_eq!(Styling::from_config_name(""), Styling::SemiFormal);
}

#[test]
fn config_names_survive_stray_case_and_whitespace() {
    assert_eq!(Styling::from_config_name("  Semi-Casual \n"), Styling::SemiCasual);
    assert_eq!(Structure::from_config_name("PROSE"), Structure::Prose);
    assert_eq!(NoteContext::from_config_name(" Email"), NoteContext::Email);
}

#[test]
fn every_config_spelling_round_trips_to_its_wire_token() {
    // Config spells these with hyphens, and so does the wire format, so the
    // two agree — but only because each spelling has an explicit arm.
    for name in ["casual", "semi-casual", "semi-formal", "formal"] {
        assert_eq!(Styling::from_config_name(name).wire(), name);
    }
    for name in ["prose", "lists"] {
        assert_eq!(Structure::from_config_name(name).wire(), name);
    }
    for name in ["general", "email"] {
        assert_eq!(NoteContext::from_config_name(name).wire(), name);
    }
}

#[test]
fn the_transcript_is_separated_from_the_control_line_by_one_newline() {
    let msg = cleanup_user_message(
        Styling::Formal,
        Structure::Lists,
        NoteContext::Email,
        "so uh remind bob about the thing",
    );
    assert_eq!(
        msg,
        "[Styling: formal] [Structure: lists] [Context: email]\nso uh remind bob about the thing"
    );
    assert_eq!(msg.lines().next().unwrap(), control_line(Styling::Formal, Structure::Lists, NoteContext::Email));
    assert_eq!(msg.matches('\n').count(), 1, "a blank line between the two is off-format");
}

#[test]
fn a_multi_line_transcript_is_passed_through_untouched() {
    // Only the first newline belongs to the format; the rest are the user's.
    let msg = cleanup_user_message(
        Styling::default(),
        Structure::default(),
        NoteContext::default(),
        "  first line\nsecond line  ",
    );
    assert!(msg.ends_with("\n  first line\nsecond line  "), "got {msg:?}");
}

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
    for category in [
        "Completed or past action",
        "Someone else's action",
        "Hypothetical or conditional",
        "Opinion, venting, emotion",
        "Observation or fact",
        "Vague aspiration or idea",
        "Rhetorical question",
    ] {
        assert!(
            extract_prompt().contains(category),
            "naming a category is what suppresses it; `{category}` is missing"
        );
    }
}

#[test]
fn the_extraction_prompt_says_an_empty_answer_is_normal() {
    let prompt = extract_prompt();
    assert!(prompt.contains("Returning an empty list is the correct and common answer"));
    assert!(prompt.contains("When uncertain, return nothing"));
}

#[test]
fn the_extraction_prompt_carries_at_least_two_hard_negative_exemplars() {
    // Positive-only exemplars teach the model that output is always
    // expected, which is the same over-triggering failure the negative
    // categories exist to prevent.
    let empty_answers = extract_prompt().matches(r#"{"tasks": []}"#).count();
    assert!(
        empty_answers >= 2,
        "expected at least two exemplars answering with an empty list, found {empty_answers}"
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
    // This bullet and exemplar name the failure directly instead of trusting
    // "verbatim" to rule it out on its own.
    let prompt = extract_prompt();
    assert!(prompt.contains("copy-paste, not a transcription"));
    assert!(prompt.contains(r#"Preserve filler words ("um", "uh")"#));
    assert!(
        prompt.contains("um so i guess we should call the vet about milo at some point"),
        "the exemplar note demonstrating filler-preserving evidence is missing"
    );
    assert!(
        prompt.contains(r#""evidence": "we should call the vet about milo at some point""#),
        "the exemplar's evidence must keep \"milo\" lowercase and drop no words, or it stops \
         demonstrating the rule it follows"
    );
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
