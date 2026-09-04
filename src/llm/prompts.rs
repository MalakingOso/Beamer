//! The input shape s1-mini was trained on. A reworded system prompt or an
//! out-of-set control-line value degrades output with no error — the server
//! still answers HTTP 200 — so the axis enums are the enforcement: there is no
//! `&str` path by which an untrained value reaches the model, and config
//! strings fall back to the trained default. No crate-rooted paths here:
//! `task_eval` path-includes this module directly.

/// System prompt, verbatim from the model card and pinned by test. Do not
/// reflow or "fix" its punctuation: that leaves the trained shape with no
/// error to trace it back from.
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
    /// The default.
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
    /// Unknown values fall back to the default; a bad config value must not
    /// produce an un-sendable request.
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

/// Control line opening every cleanup request. Enums only — no `&str`
/// overload — so an out-of-set value cannot reach the model.
pub fn control_line(styling: Styling, structure: Structure, context: NoteContext) -> String {
    format!(
        "[Styling: {}] [Structure: {}] [Context: {}]",
        styling.wire(),
        structure.wire(),
        context.wire()
    )
}

/// Control line, one newline, then the transcript verbatim. Never trimmed or
/// re-encoded here; pre-editing the input leaves the trained shape.
pub fn cleanup_user_message(
    styling: Styling,
    structure: Structure,
    context: NoteContext,
    transcript: &str,
) -> String {
    format!("{}\n{}", control_line(styling, structure, context), transcript)
}

/// Stage 2 extraction policy for Gemma — tunable prompt text, unlike
/// [`CLEANUP_SYSTEM`], but its shape is load-bearing: enumerated negatives
/// with examples, "empty is normal", two hard-negative exemplars, verbatim
/// `evidence` (rejected downstream if not), strict-or-null dates with
/// `due_phrase` kept for the picker. Aspirations excluded deliberately — a
/// poisoned list cannot be un-poisoned. Private body with `{TODAY}` filled per
/// request by [`extract_system`].
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
- "evidence" is a copy-paste, not a transcription. Preserve filler words ("um", "uh"), lowercase names, and typos exactly as written - do not fix grammar, spelling, or casing. A single changed character breaks the match and the whole task is discarded.
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
{"tasks": [{"text": "Call the vet about Milo", "evidence": "I need to call the vet about Milo", "confidence": 0.95, "due": null, "due_all_day": false, "due_phrase": null, "kind": "todo"}, {"text": "Sort the garage out", "evidence": "I'll sort the garage out sometime next week", "confidence": 0.72, "due": null, "due_all_day": false, "due_phrase": "sometime next week", "kind": "todo"}]}

Note: "um so i guess we should call the vet about milo at some point"
{"tasks": [{"text": "Call the vet about Milo", "evidence": "we should call the vet about milo at some point", "confidence": 0.85, "due": null, "due_all_day": false, "due_phrase": "at some point", "kind": "todo"}]}"#;

/// Extraction system prompt for a given day. The date goes in the system
/// message — the user message carries the note unmodified (pinned by test) —
/// and the weekday is included, since "before Friday" is unresolvable from a
/// bare ISO date. `replace`, not `format!`: the body is full of JSON braces.
pub fn extract_system(today: chrono::NaiveDate) -> String {
    EXTRACT_BODY.replace("{TODAY}", &today.format("%A, %-d %B %Y (%Y-%m-%d)").to_string())
}

#[cfg(test)]
#[path = "prompts/tests.rs"]
mod tests;
