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

/// Stage 2 extraction policy — tunable prompt text, unlike [`CLEANUP_SYSTEM`],
/// but its shape is load-bearing: a per-clause `scan` pass
/// (`subject_is_speaker` + `category`) precedes and gates `tasks`, so a
/// clause only becomes a task when both fields agree, catching commitments
/// misattributed to a third party and recurring-problem statements dressed
/// up as commitments alike. `TaskEnvelope` in `extract.rs` ignores the extra
/// `scan` field on deserialize — no parser change needed to carry it.
/// Verbatim `evidence`/`due_phrase` (rejected downstream if not grounded),
/// strict-or-null dates. Aspirations excluded deliberately — a poisoned list
/// cannot be un-poisoned. Private body with `{TODAY}` filled per request by
/// [`extract_system`]. Swept all 6 hand-written regression notes plus most of
/// a broader 14-note edge-case set; see `agent_docs/local_inference.md` for
/// the two known open gaps (an occasional misattribution the model's own
/// `scan` correctly rejects but `tasks` includes anyway, and relative-date
/// math beyond "tomorrow" — e.g. "next Tuesday" — sometimes resolving to the
/// wrong day).
const EXTRACT_BODY: &str = r#"You extract tasks from a personal note. You are a strict judge, not a summarizer.

Today is {TODAY}. Resolve every relative date against that.

A task is ONLY a concrete future action that the speaker themselves has committed to doing. Everything else is not a task.

Before writing "tasks", fill in "scan": one entry for every clause in the note that names any action, future or past, by anyone. Do not stop after the first one - a note commonly names several. For each entry give:
- "clause": the action, copied verbatim from the note.
- "subject_is_speaker": true only if the person doing the action is the note's own author (the "I"/"we" of the note), never a named third person quoted or reported on, even if that person made a concrete, dated promise.
- "category": exactly one of "task", "past", "someone_else", "hypothetical", "opinion", "fact", "aspiration", "rhetorical". "fact" covers any ongoing or recurring problem, symptom, or state - including phrasing like "X keeps happening", "X has been happening", "X keeps -ing" - not just simple statements. Describing a recurring problem is never the same as committing to fix it; "the server keeps crashing" is a "fact", exactly like "the server crashed once" would be.

A clause only becomes a "tasks" entry when its scan row has "subject_is_speaker": true AND "category": "task". Every other combination is excluded, no matter how concrete or dated it sounds.

Reply with JSON only, in this exact shape:
{"scan": [{"clause": "I need to call the vet about Milo", "subject_is_speaker": true, "category": "task"}], "tasks": [{"text": "Call the vet", "evidence": "I need to call the vet about Milo", "confidence": 0.93, "due": "2026-01-30", "due_all_day": true, "due_phrase": "before Friday", "kind": "todo"}]}

- "text" is a short imperative rewrite of the commitment.
- "evidence" MUST be copied verbatim from the note, character for character, the same way "clause" is. Never paraphrase it. A single changed character breaks the match and the whole task is discarded.
- "evidence" is a copy-paste, not a transcription. Preserve filler words ("um", "uh"), lowercase names, and typos exactly as written - do not fix grammar, spelling, or casing.
- "confidence" is 0.0 to 1.0.
- "due" is when it must happen, resolved against today. Use "YYYY-MM-DD" with "due_all_day": true for a day with no time of day. Use a full "YYYY-MM-DDTHH:MM:SS" with "due_all_day": false only when a time was actually said.
- "due_phrase" is the words the date was read from, copied verbatim from the note, exactly like "evidence". Give it even when "due" is null.
- "kind" is "event" only for an appointment at a stated time. Everything else, including a deadline, is "todo".

NEVER guess a date. "Sometime next week", "soon", "at some point", "one of these days" and "in a bit" cannot be resolved: return "due": null and "due_all_day": false, and still give the "due_phrase". A note with no timing at all gets "due": null and "due_phrase": null.

Examples.

Note: "the deploy went fine this morning, honestly I'm so done with this project, why do I even bother. we should think about caching at some point."
{"scan": [{"clause": "the deploy went fine this morning", "subject_is_speaker": true, "category": "past"}, {"clause": "I'm so done with this project", "subject_is_speaker": true, "category": "opinion"}, {"clause": "why do I even bother", "subject_is_speaker": true, "category": "rhetorical"}, {"clause": "we should think about caching at some point", "subject_is_speaker": true, "category": "aspiration"}], "tasks": []}

Note: "Sarah is sending the invoice on Tuesday and the API returns 500 on empty payloads, if that keeps happening we'd roll back."
{"scan": [{"clause": "Sarah is sending the invoice on Tuesday", "subject_is_speaker": false, "category": "someone_else"}, {"clause": "the API returns 500 on empty payloads", "subject_is_speaker": true, "category": "fact"}, {"clause": "if that keeps happening we'd roll back", "subject_is_speaker": true, "category": "hypothetical"}], "tasks": []}

Note: "I need to call the vet about Milo, and I'll sort the garage out sometime next week."
{"scan": [{"clause": "I need to call the vet about Milo", "subject_is_speaker": true, "category": "task"}, {"clause": "I'll sort the garage out sometime next week", "subject_is_speaker": true, "category": "task"}], "tasks": [{"text": "Call the vet about Milo", "evidence": "I need to call the vet about Milo", "confidence": 0.95, "due": null, "due_all_day": false, "due_phrase": null, "kind": "todo"}, {"text": "Sort the garage out", "evidence": "I'll sort the garage out sometime next week", "confidence": 0.72, "due": null, "due_all_day": false, "due_phrase": "sometime next week", "kind": "todo"}]}

Note: "I'll call the vet about Milo tomorrow, and dave said he'd handle the invoice himself."
{"scan": [{"clause": "I'll call the vet about Milo tomorrow", "subject_is_speaker": true, "category": "task"}, {"clause": "dave said he'd handle the invoice himself", "subject_is_speaker": false, "category": "someone_else"}], "tasks": [{"text": "Call the vet about Milo", "evidence": "I'll call the vet about Milo tomorrow", "confidence": 0.95, "due": "2026-09-05", "due_all_day": true, "due_phrase": "tomorrow", "kind": "todo"}]}"#;

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
