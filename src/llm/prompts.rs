//! The wire format `superwhisper/s1-mini` was trained on.
//!
//! s1-mini is a 0.6B text normalizer, not a chat model. Its publisher documents
//! the input shape as a trained contract: a reworded system prompt, or a
//! control-line value outside the trained sets, produces hallucinated or
//! garbled output. There is nothing to catch — the server still answers HTTP
//! 200 with a plausible-looking body, so a malformed request is indistinguishable
//! from a good one at the transport layer.
//!
//! So the type system is the enforcement. [`control_line`] takes only the three
//! axis enums; there is deliberately no `&str`-accepting path by which an
//! out-of-set value could reach the model. Config strings are funnelled through
//! `from_config_name`, which falls back to the trained default rather than
//! forwarding whatever was typed.
//!
//! Nothing here may use a crate-rooted path — `src/bin/task_eval.rs` `#[path]`-includes
//! `../llm/mod.rs` directly, and there is no `src/lib.rs` to resolve such a path
//! against.

/// The system prompt, verbatim from the model card.
///
/// Pinned by an exact-equality test for the same reason [`super::MODEL_CREDIT`]
/// is: the risk is not malice but tidiness. Reflowing this sentence, or
/// "fixing" its semicolon, moves the input off the shape the model was trained
/// on and degrades every cleanup pass with no error to trace it back from.
pub const CLEANUP_SYSTEM: &str = "You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.";

/// How formal the cleaned text should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Styling {
    Casual,
    SemiCasual,
    /// The publisher's suggested default: standard written English,
    /// contractions kept, colloquialisms smoothed.
    #[default]
    SemiFormal,
    Formal,
}

/// Whether the model may turn an enumeration into bullets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Structure {
    Prose,
    /// The default. A dictated note that enumerates things should become
    /// bullets — though in practice the model is conservative enough that it
    /// usually returns prose anyway. See `agent_docs/local_inference.md`.
    #[default]
    Lists,
}

/// Named `NoteContext` rather than `Context` so it does not collide with
/// `anyhow::Context`, which is imported across this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteContext {
    #[default]
    General,
    Email,
}

impl Styling {
    /// Mirrors `NoteColor::from_config_name`: an unrecognized value falls back
    /// to the default instead of panicking or being passed through. A bad
    /// config value must not produce an un-sendable request.
    pub fn from_config_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "casual" => Self::Casual,
            "semi-casual" => Self::SemiCasual,
            "formal" => Self::Formal,
            _ => Self::SemiFormal,
        }
    }

    /// The exact token the model was trained on.
    pub fn wire(&self) -> &'static str {
        match self {
            Self::Casual => "casual",
            Self::SemiCasual => "semi-casual",
            Self::SemiFormal => "semi-formal",
            Self::Formal => "formal",
        }
    }
}

impl Structure {
    /// See [`Styling::from_config_name`].
    pub fn from_config_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "prose" => Self::Prose,
            _ => Self::Lists,
        }
    }

    /// The exact token the model was trained on.
    pub fn wire(&self) -> &'static str {
        match self {
            Self::Prose => "prose",
            Self::Lists => "lists",
        }
    }
}

impl NoteContext {
    /// See [`Styling::from_config_name`].
    pub fn from_config_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "email" => Self::Email,
            _ => Self::General,
        }
    }

    /// The exact token the model was trained on.
    pub fn wire(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Email => "email",
        }
    }
}

/// The control line that must open every cleanup request.
///
/// Takes enums and nothing else. That is the whole point of this module: there
/// is no overload, no `&str` variant, no escape hatch, so an out-of-set value
/// cannot be constructed at a call site and reach the model.
pub fn control_line(styling: Styling, structure: Structure, context: NoteContext) -> String {
    format!(
        "[Styling: {}] [Structure: {}] [Context: {}]",
        styling.wire(),
        structure.wire(),
        context.wire()
    )
}

/// Control line, one newline, then the transcript verbatim.
///
/// The transcript is never trimmed or re-encoded here — cleanup is the model's
/// job, and pre-editing its input is one more way to leave the trained shape.
pub fn cleanup_user_message(
    styling: Styling,
    structure: Structure,
    context: NoteContext,
    transcript: &str,
) -> String {
    format!("{}\n{}", control_line(styling, structure, context), transcript)
}

/// Stage 2 — the task-extraction policy.
///
/// Unlike [`CLEANUP_SYSTEM`] this is **not** a wire format handed down by a
/// publisher: Gemma is a general model and this is a prompt, tunable against a
/// corpus. What is not tunable is its shape, and every part of it is load
/// bearing:
///
/// - **The negative categories are enumerated, with examples.** Naming a
///   category is what suppresses it. An extraction-shaped instruction ("list
///   the action items") biases the model toward producing output, which is
///   precisely the failure to avoid — a note that is pure venting must yield
///   nothing.
/// - **It states that empty is normal.** Without that sentence models reliably
///   invent a task rather than return none.
/// - **Two of the three exemplars are hard negatives** returning an empty
///   list. Positive-only exemplars teach the model that output is always
///   expected, which is the same failure by another route.
/// - **`evidence` is required to be verbatim.** A task whose evidence is not
///   in the note is a fabrication, and `extract::parse_tasks` rejects it.
///
/// Aspirations are excluded deliberately. They are the largest ambiguous class
/// and admitting them is what turns a task list into a graveyard of vague
/// intentions. The policy can be loosened later; a list nobody trusts cannot be
/// un-poisoned.
pub const EXTRACT_SYSTEM: &str = r#"You extract tasks from a personal note. You are a strict judge, not a summarizer.

A task is ONLY a concrete future action that the speaker has committed to doing themselves. Everything else is not a task.

These are NOT tasks:
- Completed or past action - "I called the vet yesterday"
- Someone else's action - "Sarah is sending the invoice"
- Hypothetical or conditional - "if the build fails we'd roll back"
- Opinion, venting, emotion - "I'm so done with this project"
- Observation or fact - "the API returns 500 on empty payloads"
- Vague aspiration or idea - "we should think about caching", "it'd be cool to have dark mode"
- Rhetorical question - "why do I even bother"

Most notes contain no tasks. Returning an empty list is the correct and common answer. When uncertain, return nothing. Prefer omitting a task over inventing one.

Reply with JSON only, in this exact shape:
{"tasks": [{"text": "Call the vet", "evidence": "I need to call the vet about Milo", "confidence": 0.93}]}

- "text" is a short imperative rewrite of the commitment.
- "evidence" MUST be copied verbatim from the note, character for character. Never paraphrase it.
- "confidence" is 0.0 to 1.0.

Examples.

Note: "the deploy went fine this morning, honestly I'm so done with this project, why do I even bother. we should think about caching at some point."
{"tasks": []}

Note: "Sarah is sending the invoice on Tuesday and the API returns 500 on empty payloads, if that keeps happening we'd roll back."
{"tasks": []}

Note: "I need to call the vet about Milo tomorrow, and I'll send Tuesday's invoice before Friday."
{"tasks": [{"text": "Call the vet about Milo", "evidence": "I need to call the vet about Milo tomorrow", "confidence": 0.95}, {"text": "Send Tuesday's invoice", "evidence": "I'll send Tuesday's invoice before Friday", "confidence": 0.92}]}"#;

#[cfg(test)]
mod tests {
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
                EXTRACT_SYSTEM.contains(category),
                "naming a category is what suppresses it; `{category}` is missing"
            );
        }
    }

    #[test]
    fn the_extraction_prompt_says_an_empty_answer_is_normal() {
        assert!(EXTRACT_SYSTEM
            .contains("Returning an empty list is the correct and common answer"));
        assert!(EXTRACT_SYSTEM.contains("When uncertain, return nothing"));
    }

    #[test]
    fn the_extraction_prompt_carries_at_least_two_hard_negative_exemplars() {
        // Positive-only exemplars teach the model that output is always
        // expected, which is the same over-triggering failure the negative
        // categories exist to prevent.
        let empty_answers = EXTRACT_SYSTEM.matches(r#"{"tasks": []}"#).count();
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
        assert!(EXTRACT_SYSTEM.contains("copied verbatim from the note"));
    }
}
