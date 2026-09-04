//! The shape of an extracted task. Types only, no persistence.
//! Each row is both a chip the user acts on and a labelled eval-corpus example.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

/// Where a proposal stands with the user. `Suggested` is not a task yet:
/// nothing enters a list unconfirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Suggested,
    Accepted,
    Dismissed,
}

/// Old rows with no `status` key were never decided; only `Suggested` avoids
/// fabricating a label in the corpus.
impl Default for TaskStatus {
    fn default() -> Self {
        Self::Suggested
    }
}


/// Whether a dated task is a deadline (`VTODO`+`DUE`) or an appointment
/// (`VEVENT`+`DTSTART`/`DTEND`) at `.ics` export. `Todo` stays the default so
/// pre-`kind` rows and unrecognised values degrade to a to-do, never nothing.
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
    /// A bare `YYYY-MM-DD`; no time is invented for one.
    AllDay(NaiveDate),
    At(NaiveDateTime),
}

impl Due {
    pub fn date(&self) -> NaiveDate {
        match self {
            Self::AllDay(d) => *d,
            Self::At(dt) => dt.date(),
        }
    }
}

/// Parse a stored `due` value. Tolerant of either shape regardless of `all_day`.
/// RFC3339 converts to local wall-clock: "nine on Tuesday" is nine wherever read.
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
    // Bare local datetimes: `<input type="datetime-local">` output, or the model omitting an offset.
    NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(due, "%Y-%m-%dT%H:%M"))
        .ok()
        .map(Due::At)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// The note this was extracted from.
    pub note_id: String,
    /// Normalized imperative shown on the chip, e.g. "Call the vet".
    pub text: String,
    /// The exact note span that produced `text`: what makes a false positive
    /// obvious at a glance and a bad extraction debuggable after the fact.
    pub evidence: String,
    /// Confidence as reported by the model, expected in 0.0-1.0 but never
    /// clamped: an out-of-range value is a misfire the corpus should keep.
    pub confidence: f32,
    #[serde(default)]
    pub status: TaskStatus,
    /// Only meaningful when `status == Accepted`; a suggestion is not a task yet.
    #[serde(default)]
    pub done: bool,
    /// RFC3339, when the model proposed it.
    pub created: String,
    /// RFC3339, when the user accepted or dismissed it. The timestamp (paired
    /// with `created`) is the eval signal, so a decided row always carries one.
    #[serde(default)]
    pub decided: Option<String>,
    /// When it is due: RFC3339, or bare `YYYY-MM-DD` when `due_all_day`. `None`
    /// when no date was found or the phrase could not be resolved. A `String`
    /// like `created`/`decided`, parsed at render/export via [`parse_due`].
    #[serde(default)]
    pub due: Option<String>,
    /// Whether `due` names a day rather than a moment.
    #[serde(default)]
    pub due_all_day: bool,
    /// The literal note span the date was read from — present even when `due`
    /// is `None`, so the user can resolve it in one click. Grounded against
    /// the note by `llm::extract`; a phrase not in the note is an invented date.
    #[serde(default)]
    pub due_phrase: Option<String>,
    #[serde(default)]
    pub kind: TaskKind,
}

impl Task {
    pub fn due_parsed(&self) -> Option<Due> {
        parse_due(self.due.as_deref()?, self.due_all_day)
    }

    /// Whether the due day is past. Compares days, not instants: due at 09:00
    /// today is not overdue at 17:00. An undated task is never overdue.
    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        !self.done && self.due_parsed().is_some_and(|d| d.date() < today)
    }
}

/// The model's half of a row, before the store gives it identity and time.
/// One struct so a row can never be written half-dated.
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
