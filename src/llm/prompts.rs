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
/// - **Dates are resolved strictly or not at all.** The prompt names the vague
///   phrasings explicitly and requires `due: null` for them, because a model
///   asked for a date will produce one. `due_phrase` is still returned in that
///   case, which is what turns "I could not resolve this" into a date picker
///   rather than a shrug.
///
/// Aspirations are excluded deliberately. They are the largest ambiguous class
/// and admitting them is what turns a task list into a graveyard of vague
/// intentions. The policy can be loosened later; a list nobody trusts cannot be
/// un-poisoned.
///
/// Held as a body with `{TODAY}` still to fill in, private, and reached only
/// through [`extract_system`]. The date is the one part that changes per
/// request; everything else is fixed text that `prompts/tests.rs` pins category
/// by category.
const EXTRACT_BODY: &str = r#"You extract tasks from a personal note. You are a strict judge, not a summarizer.

Today is {TODAY}. Resolve every relative date against that.

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
{"tasks": [{"text": "Call the vet", "evidence": "I need to call the vet about Milo", "confidence": 0.93, "due": "2026-01-30", "due_all_day": true, "due_phrase": "before Friday", "kind": "todo"}]}

- "text" is a short imperative rewrite of the commitment.
- "evidence" MUST be copied verbatim from the note, character for character. Never paraphrase it.
- "confidence" is 0.0 to 1.0.
- "due" is when it must happen, resolved against today. Use "YYYY-MM-DD" with "due_all_day": true for a day with no time of day. Use a full "YYYY-MM-DDTHH:MM:SS" with "due_all_day": false only when a time was actually said.
- "due_phrase" is the words the date was read from, copied verbatim from the note, exactly like "evidence". Give it even when "due" is null.
- "kind" is "event" only for an appointment at a stated time. Everything else, including a deadline, is "todo".

NEVER guess a date. "Sometime next week", "soon", "at some point", "one of these days" and "in a bit" cannot be resolved: return "due": null and "due_all_day": false, and still give the "due_phrase". A note with no timing at all gets "due": null and "due_phrase": null.

Examples.

Note: "the deploy went fine this morning, honestly I'm so done with this project, why do I even bother. we should think about caching at some point."
{"tasks": []}

Note: "Sarah is sending the invoice on Tuesday and the API returns 500 on empty payloads, if that keeps happening we'd roll back."
{"tasks": []}

Note: "I need to call the vet about Milo, and I'll sort the garage out sometime next week."
{"tasks": [{"text": "Call the vet about Milo", "evidence": "I need to call the vet about Milo", "confidence": 0.95, "due": null, "due_all_day": false, "due_phrase": null, "kind": "todo"}, {"text": "Sort the garage out", "evidence": "I'll sort the garage out sometime next week", "confidence": 0.72, "due": null, "due_all_day": false, "due_phrase": "sometime next week", "kind": "todo"}]}"#;

/// The extraction system prompt for a given day.
///
/// ⚠️ **The date goes in the system message, never the user message.** The note
/// is still sent as-is — `extract::build_request` carries it unmodified, and a
/// test pins that — so nothing about the transcript changes.
///
/// ⚠️ **Safe here and only here.** Gemma is a general instruct model and its
/// prompt is tunable text. The same move against s1-mini would be the exact
/// out-of-distribution failure the module docs above warn about: cleanup's
/// prompt is a trained wire format and is not touched.
///
/// The **weekday** is included, not just the ISO date. "Before Friday" is not
/// resolvable from `2026-08-23` alone, and asking a language model to compute a
/// day of the week is asking it to be wrong.
pub fn extract_system(today: chrono::NaiveDate) -> String {
    // `replace`, not `format!`: the prompt is full of JSON braces, and a format
    // string would have to escape every one of them.
    EXTRACT_BODY.replace("{TODAY}", &today.format("%A, %-d %B %Y (%Y-%m-%d)").to_string())
}

#[cfg(test)]
#[path = "prompts/tests.rs"]
mod tests;
