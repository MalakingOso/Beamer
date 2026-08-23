//! The shape of an extracted task.
//!
//! Types only — nothing here knows about persistence, mirroring the split
//! between `model.rs` and `mod.rs`.
//!
//! Every row is two things at once: a chip the user acts on, and a labelled
//! example for the eval corpus that will later measure extraction. The second
//! job is why several fields exist that a to-do list would not need.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// Where a proposal stands with the user.
///
/// `Suggested` is not a to-do item yet. Nothing enters a task list unconfirmed
/// — the whole point of the accept/dismiss pair is that the model proposes and
/// the user decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Suggested,
    Accepted,
    Dismissed,
}

/// A row loaded from an older `tasks.json` with no `status` key is a row the
/// user never decided on. Defaulting to `Accepted` would invent confirmations
/// that never happened; defaulting to `Dismissed` would invent rejections. Both
/// are fabricated labels in the corpus, so the undecided state is the only safe
/// default.
impl Default for TaskStatus {
    fn default() -> Self {
        Self::Suggested
    }
}


/// Whether a dated task is a deadline or an appointment.
///
/// The model decides, per task, and the distinction is not cosmetic: it is what
/// an `.ics` export emits — `VTODO` with a `DUE` for a deadline, `VEVENT` with
/// a `DTSTART`/`DTEND` for an appointment. "Send the invoice before Friday" is
/// a to-do; "standup at 9 on Tuesday" is an event.
///
/// `Todo` is the default, and must stay the default: a row loaded from a
/// `tasks.json` written before this field existed is an extracted commitment,
/// which is a to-do. It is also what an unrecognised `kind` degrades to, so a
/// model inventing a third category costs a calendar shape and never the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    Todo,
    Event,
}

/// What a stored `due` string actually means, once parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// A bare `YYYY-MM-DD`. "Before Friday" has no time of day and inventing
    /// one would be a guess the user never made.
    AllDay(NaiveDate),
    At(NaiveDateTime),
}

impl Due {
    /// The day it falls on, for sorting and for the overdue test.
    pub fn date(&self) -> NaiveDate {
        match self {
            Self::AllDay(d) => *d,
            Self::At(dt) => dt.date(),
        }
    }
}

/// Parse a stored `due` value.
///
/// Deliberately tolerant of both shapes regardless of `all_day`: the flag says
/// how the value was *meant*, and a row hand-edited into `tasks.json` with the
/// other shape should still render rather than silently vanish.
///
/// RFC3339 is converted to **local** wall-clock time. What the user said was
/// "nine on Tuesday", and nine is nine wherever they are reading it.
pub fn parse_due(due: &str, all_day: bool) -> Option<Due> {
    let due = due.trim();
    if let Ok(date) = NaiveDate::parse_from_str(due, "%Y-%m-%d") {
        return Some(Due::AllDay(date));
    }
    if all_day {
        // A datetime stored under an all-day flag still means that day.
        if let Ok(dt) = DateTime::parse_from_rfc3339(due) {
            return Some(Due::AllDay(dt.with_timezone(&Local).date_naive()));
        }
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(due) {
        return Some(Due::At(dt.with_timezone(&Local).naive_local()));
    }
    // What `<input type="datetime-local">` submits, and what the model returns
    // when it omits an offset it was never given.
    NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M"))
        .ok()
        .map(Due::At)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// Provenance. Click a task, get the note it came from — and, when the
    /// extraction misfires, the note is what you need to read to see why.
    pub note_id: String,
    /// Normalized imperative, e.g. "Call the vet". This is what the chip shows.
    pub text: String,
    /// The exact span of the note that produced `text`.
    ///
    /// Not decoration, and not a duplicate of `text`. A false positive is
    /// obvious in one glance when the span that triggered it sits next to it —
    /// "I should probably call the vet sometime" reads very differently as an
    /// extracted commitment than "Call the vet" does alone. It is also the only
    /// thing that makes the prompt debuggable after the fact: without the span,
    /// a bad extraction is a verdict with no evidence, and tuning the prompt
    /// becomes guesswork against a note that may since have been edited.
    pub evidence: String,
    /// Confidence as reported by the model, expected in 0.0-1.0.
    ///
    /// Expected, **not enforced.** A value outside the range means the prompt
    /// or the parser misfired, and that is exactly the anomaly the corpus
    /// should preserve rather than quietly flatten. Clamping belongs at render
    /// time, where it affects a progress bar and nothing else.
    pub confidence: f32,
    #[serde(default)]
    pub status: TaskStatus,
    /// Only meaningful when `status == Accepted`. A suggestion cannot be
    /// completed, because it is not a task yet.
    #[serde(default)]
    pub done: bool,
    /// RFC3339, when the model proposed it.
    pub created: String,
    /// RFC3339, when the user accepted or dismissed it.
    ///
    /// `Option<String>` rather than a bool because the *timestamp* is the eval
    /// signal, not the fact of a decision. Paired with `created` it gives how
    /// long the user looked at the proposal before rejecting it — an instant
    /// dismissal and a considered one are different labels. A dismissed row
    /// with no decision time is a row that cannot be used as a labelled
    /// negative at all, so the two must be recorded together or not at all.
    #[serde(default)]
    pub decided: Option<String>,
    /// When it is due: RFC3339, or a bare `YYYY-MM-DD` when `due_all_day`.
    ///
    /// `None` when the model found no date **or** found a phrase it could not
    /// resolve. A `String` rather than a `chrono` type on purpose: `created`
    /// and `decided` already store RFC3339 strings, the corpus stays readable
    /// by eye, and `task_eval` keeps parsing `tasks.json` without a schema
    /// change. Parsing happens at render and export time, through [`parse_due`].
    #[serde(default)]
    pub due: Option<String>,
    /// Whether `due` names a day rather than a moment.
    #[serde(default)]
    pub due_all_day: bool,
    /// The literal span of the note the date was read from — **present even
    /// when `due` is `None`**.
    ///
    /// That asymmetry is the point. "Sometime next week" is not resolvable and
    /// must never be guessed at, but showing the user the phrase the model saw,
    /// beside a date picker, turns a dead end into one click. A shrug would
    /// throw away the only thing that makes the fix obvious.
    ///
    /// Grounded against the note by `llm::extract`, the same check `evidence`
    /// gets: a phrase that is not in the note is an invented date.
    #[serde(default)]
    pub due_phrase: Option<String>,
    /// Deadline or appointment. See [`TaskKind`].
    #[serde(default)]
    pub kind: TaskKind,
}

impl Task {
    /// The due value, parsed. `None` when there is no date or it does not parse.
    pub fn due_parsed(&self) -> Option<Due> {
        parse_due(self.due.as_deref()?, self.due_all_day)
    }

    /// Whether the due day is already past. An undated task is never overdue.
    ///
    /// Compares **days**, not instants: a task due at 09:00 today is not
    /// overdue at 17:00 in the sense the user means, and reddening it would
    /// train them to ignore the colour.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.done && self.due_parsed().is_some_and(|d| d.date() < today)
    }
}

/// The model's half of a row, before the store gives it identity and time.
///
/// Constructed in one place so a row can never be written half-dated — the
/// failure mode of setting the fields after construction, which is easy to
/// forget and produces a task whose chip says nothing and whose export is
/// empty.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Proposal {
    pub text: String,
    pub evidence: String,
    pub confidence: f32,
    pub due: Option<String>,
    pub due_all_day: bool,
    pub due_phrase: Option<String>,
    pub kind: TaskKind,
}
