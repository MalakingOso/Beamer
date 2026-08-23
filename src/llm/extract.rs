//! Stage 2 — task extraction with Gemma.
//!
//! Extraction is a **precision** problem, not an extraction problem: a
//! fabricated task is worse than a missed one, because a list nobody trusts
//! cannot be un-poisoned. The prompt in `prompts.rs` does what a prompt can;
//! everything here is the part that does not depend on the model behaving.
//!
//! Three checks, all of them at parse time:
//!
//! 1. **Fences are tolerated.** Models emit ```` ```json ```` regardless of
//!    instructions, and the server's `json_object` grammar constraint is not
//!    honoured by every version.
//! 2. **Evidence must be present in the note.** A proposal whose evidence span
//!    is not in the text that was actually sent is a fabrication and is
//!    dropped. This is the one check that does not care how good the model is.
//! 3. **The confidence floor is applied here, not in the UI.** A row that is
//!    never shown to anyone is not a labelled example and must not reach
//!    `tasks.json`, where it would pollute the eval corpus with a decision
//!    nobody made.
//!
//! Dates get four more gates, in [`resolve_date`]. Every one of them
//! **downgrades rather than discards**: a date the model got wrong costs the
//! date, never the task. Losing a real commitment because its date failed to
//! parse would be the worst possible trade.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, NaiveDateTime};
use serde::Deserialize;

use super::chat::{self, ChatError, ChatRequest, Message, ResponseFormat};
use super::prompts;
use super::ExtractConfig;

/// Whether a dated task is a deadline or an appointment.
///
/// Mirrors `notes::task::TaskKind`, and is deliberately a separate type for the
/// same reason [`ProposedTask`] is: this module cannot reach into the crate (see
/// the module rule in `prompts.rs`), and the store owns the persisted shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskKind {
    #[default]
    Todo,
    Event,
}

/// One task the model proposed, after grounding and the confidence floor.
///
/// Deliberately not `notes::task::Task`: this module cannot reach into the
/// crate (see the module rule in `prompts.rs`), and the store owns identity,
/// status and timestamps. This is the model's half of the record and nothing
/// more.
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedTask {
    pub text: String,
    /// The span of the note that produced this task, verbatim. What makes a
    /// false positive visible in one glance.
    pub evidence: String,
    pub confidence: f32,
    /// `YYYY-MM-DD` when `due_all_day`, otherwise `YYYY-MM-DDTHH:MM:SS`.
    /// `None` when the model found no date, or found one that failed a gate.
    pub due: Option<String>,
    pub due_all_day: bool,
    /// The verbatim span the date was read from. Survives when `due` does not —
    /// that asymmetry is what lets the UI offer a picker instead of a shrug.
    pub due_phrase: Option<String>,
    pub kind: TaskKind,
}

#[derive(Deserialize)]
struct TaskEnvelope {
    #[serde(default)]
    tasks: Vec<RawTask>,
}

#[derive(Deserialize)]
struct RawTask {
    #[serde(default)]
    text: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    due: Option<String>,
    /// The model's `due_all_day` flag is deliberately **not** read. Whether a
    /// value names a day or a moment is decided by the shape of `due` itself in
    /// [`parse_due`], because the two can contradict each other — a model
    /// answering `"2026-08-25T09:00:00"` with `due_all_day: true` has said
    /// something about a time and then denied it, and the value is the half
    /// that carries the information. Serde ignores the extra key.
    #[serde(default)]
    due_phrase: Option<String>,
    /// Free text, not an enum: an unrecognised value must degrade to `Todo`,
    /// and a `#[serde(other)]` variant would make that a parse concern rather
    /// than a validation one — a malformed `kind` would take the whole envelope
    /// down with it and cost the user every task in the note.
    #[serde(default)]
    kind: Option<String>,
}

/// A validated date, or as much of one as survived.
#[derive(Debug, Clone, PartialEq, Default)]
struct ResolvedDate {
    due: Option<String>,
    all_day: bool,
    phrase: Option<String>,
}

/// Apply the four date gates. Never fails — the worst outcome is no date.
///
/// 1. **`due_phrase` must ground in the note**, via the same normalize-and-
///    contains check `evidence` gets. This is the strongest available guard
///    against an invented date and it costs nothing new: a model that made the
///    date up has to have made the phrase up too.
/// 2. **`due` must parse.** Unparseable keeps the phrase and drops the date.
/// 3. **`due` may not be more than a day in the past.** A model resolving
///    "Friday" against the wrong year is the classic failure here, and it is
///    otherwise completely silent — the task looks fine and is filed under
///    2025. One day of slack, not zero, so a pass running just after midnight
///    on something due "today" is not thrown away.
/// 4. **An unrecognised `kind` becomes `Todo`.** Handled by the caller.
///
/// `today` is passed in rather than read from the clock so every gate is
/// testable without mocking time.
fn resolve_date(raw: &RawTask, note: &str, today: NaiveDate) -> ResolvedDate {
    let phrase = raw
        .due_phrase
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let ok = is_grounded(p, note);
            if !ok {
                tracing::warn!(
                    "extraction: dropped ungrounded due_phrase {:?} on {:?}",
                    p, raw.text
                );
            }
            ok
        })
        .map(str::to_string);

    // Gate 1 also gates the date: a date read from a phrase that is not in the
    // note was read from nothing.
    if phrase.is_none() {
        return ResolvedDate::default();
    }

    let Some(due) = raw.due.as_deref().map(str::trim).filter(|d| !d.is_empty()) else {
        return ResolvedDate { due: None, all_day: false, phrase };
    };

    let Some((normalized, all_day, day)) = parse_due(due) else {
        tracing::warn!("extraction: {:?} is not a date, keeping the phrase only", due);
        return ResolvedDate { due: None, all_day: false, phrase };
    };

    if day < today - ChronoDuration::days(1) {
        tracing::warn!(
            "extraction: {:?} resolved to {}, which is in the past — keeping the phrase only",
            phrase, day
        );
        return ResolvedDate { due: None, all_day: false, phrase };
    }

    ResolvedDate { due: Some(normalized), all_day, phrase }
}

/// Parse the three shapes the model is asked for, plus RFC3339 in case it adds
/// an offset it was never given.
///
/// Returns the value normalized to what the store holds, whether it names a
/// whole day, and the day it falls on.
fn parse_due(due: &str) -> Option<(String, bool, NaiveDate)> {
    if let Ok(date) = NaiveDate::parse_from_str(due, "%Y-%m-%d") {
        return Some((date.format("%Y-%m-%d").to_string(), true, date));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(due) {
        let local = dt.with_timezone(&chrono::Local).naive_local();
        return Some((local.format("%Y-%m-%dT%H:%M:%S").to_string(), false, local.date()));
    }
    let dt = NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M"))
        .ok()?;
    Some((dt.format("%Y-%m-%dT%H:%M:%S").to_string(), false, dt.date()))
}

/// Gate 4. Anything the prompt did not ask for is a to-do.
fn parse_kind(kind: Option<&str>) -> TaskKind {
    match kind.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("event") => TaskKind::Event,
        Some("todo") | None => TaskKind::Todo,
        Some(other) => {
            tracing::debug!("extraction: unrecognised kind {:?}, treating as a to-do", other);
            TaskKind::Todo
        }
    }
}

/// Strip a markdown code fence, if the model wrapped its answer in one.
///
/// Tolerant rather than strict: an unterminated fence still yields the body,
/// because a truncated response with a usable first task is worth more than an
/// error.
pub fn strip_fences(body: &str) -> &str {
    let trimmed = body.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop the info string (```json, ```JSON, ``` on its own) up to the newline.
    let rest = match rest.find('\n') {
        Some(nl) => &rest[nl + 1..],
        None => rest,
    };
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Case- and whitespace-insensitive containment.
///
/// Not a raw `contains`. A transcript carries newlines and doubled spaces that
/// a model quoting from it will not reproduce exactly, and rejecting a genuine
/// span over a capital letter would make the grounding check look broken while
/// being useless. Normalizing this far cannot turn a sentence the note does not
/// contain into one it does, so the guard keeps its teeth.
fn is_grounded(evidence: &str, note: &str) -> bool {
    // An empty span is not grounding — every string contains the empty string,
    // so without this check a model that omitted `evidence` entirely would sail
    // through the one test that does not depend on its judgment.
    if evidence.trim().is_empty() {
        return false;
    }
    normalize(note).contains(&normalize(evidence))
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Turn a completion body into the proposals worth showing.
///
/// `note` is the text the request actually carried — grounding is checked
/// against that, not against whatever the note says by the time this returns.
///
/// An empty list is a **successful** answer and a frequent one; most notes
/// contain no tasks.
pub fn parse_tasks(
    body: &str,
    note: &str,
    min_confidence: f32,
    today: NaiveDate,
) -> Result<Vec<ProposedTask>, ChatError> {
    let json = strip_fences(body);
    let envelope: TaskEnvelope =
        serde_json::from_str(json).map_err(|e| ChatError::Malformed(e.to_string()))?;

    let mut kept = Vec::new();
    for raw in envelope.tasks {
        if raw.text.trim().is_empty() {
            tracing::debug!("extraction: dropped a proposal with no text");
            continue;
        }
        if !is_grounded(&raw.evidence, note) {
            tracing::warn!(
                "extraction: dropped ungrounded proposal {:?} (evidence {:?} is not in the note)",
                raw.text,
                raw.evidence
            );
            continue;
        }
        // Clamped rather than rejected: a model reporting 1.2 is miscalibrated,
        // not lying about the task, and the floor is a backstop anyway — the
        // accept/dismiss step is the real precision mechanism.
        let confidence = raw.confidence.clamp(0.0, 1.0);
        if confidence < min_confidence {
            tracing::debug!("extraction: dropped {:?} below the floor", raw.text);
            continue;
        }
        // The date is resolved last, and can only ever remove itself. By the
        // time control reaches here the task has already earned its place.
        let date = resolve_date(&raw, note, today);
        kept.push(ProposedTask {
            text: raw.text.trim().to_string(),
            evidence: raw.evidence.trim().to_string(),
            confidence,
            due: date.due,
            due_all_day: date.all_day,
            due_phrase: date.phrase,
            kind: parse_kind(raw.kind.as_deref()),
        });
    }
    Ok(kept)
}

/// Build the request for one note.
///
/// Asks for `json_object`, so the server's grammar constraint makes the answer
/// structurally valid by construction rather than by hope. `strip_fences`
/// stays anyway — the constraint is not honoured by every server version, and
/// the eval harness may point at one that is not.
/// `today` reaches the model in the **system** message. The note itself is
/// still sent byte for byte, which is what keeps the transcript the only thing
/// the user message ever carries.
pub fn build_request(cfg: &ExtractConfig, note: &str, today: NaiveDate) -> ChatRequest {
    ChatRequest {
        model: cfg.model.clone(),
        messages: vec![
            Message::system(prompts::extract_system(today)),
            Message::user(note),
        ],
        response_format: Some(ResponseFormat::json_object()),
    }
}

/// Run one extraction pass.
/// `today` is a parameter rather than a clock read so the validation gates are
/// testable without mocking time — and so `task_eval` can grade a note against
/// the day it was *captured*, which is the only day its relative dates ever
/// meant anything against.
pub async fn extract(
    base_url: &str,
    cfg: &ExtractConfig,
    note: &str,
    today: NaiveDate,
    timeout: Duration,
) -> Result<Vec<ProposedTask>, ChatError> {
    let request = build_request(cfg, note, today);
    let body = chat::complete(base_url, &request, timeout).await?;
    parse_tasks(&body, note, cfg.min_confidence, today)
}

#[cfg(test)]
#[path = "extract/tests.rs"]
mod tests;
