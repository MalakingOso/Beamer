//! The "done today" log: things the user got done, one short line each.
//!
//! Stored as `Note`s of `NoteKind::Accomplishment`, so they persist, sync and
//! delete through the note corpus with no new file or document root (the
//! synced document's genesis holds only `notes` and `tasks`). They never get a
//! window, extraction, or a board card: `NoteStore`'s board queries filter on
//! `Note::is_note`, and `create_kind` stamps `StageState::Skipped`.
//!
//! A record belongs to the local calendar day of `created`, which an edit never
//! moves. Day boundaries are midnight local time, not a rolling 24 hours.

use chrono::{DateTime, Local, NaiveDate};

use super::task_store::TaskStore;
use super::{Note, NoteColor, NoteKind, NoteOrigin, NoteStore};

impl Note {
    /// The local calendar day this record was made on, or `None` if `created`
    /// is not RFC3339 (such a row is left out of every day rather than guessed at).
    pub fn local_day(&self) -> Option<NaiveDate> {
        DateTime::parse_from_rfc3339(&self.created)
            .ok()
            .map(|dt| dt.with_timezone(&Local).date_naive())
    }
}

impl NoteStore {
    /// Add a line to today's log and return its id. Empty or all-whitespace
    /// text is refused, not stored. The caller flushes: a finished-thing line
    /// is a discrete gesture, so it goes through `flush_stores` inline.
    pub fn log_accomplishment(&mut self, text: &str) -> Option<String> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        Some(self.create_kind(
            text.to_string(),
            NoteColor::Purple,
            NoteOrigin::Typed,
            NoteKind::Accomplishment,
        ))
    }

    /// The accomplishments made on `day`, in the order they happened.
    pub fn accomplishments_on(&self, day: NaiveDate) -> Vec<&Note> {
        let mut rows: Vec<&Note> = self
            .notes
            .iter()
            .filter(|n| n.kind == NoteKind::Accomplishment && n.local_day() == Some(day))
            .collect();
        rows.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
        rows
    }

    /// Every day with at least one accomplishment, newest first.
    pub fn accomplishment_days(&self) -> Vec<NaiveDate> {
        let mut days: Vec<NaiveDate> = self
            .notes
            .iter()
            .filter(|n| n.kind == NoteKind::Accomplishment)
            .filter_map(Note::local_day)
            .collect();
        days.sort_unstable_by(|a, b| b.cmp(a));
        days.dedup();
        days
    }
}

/// One line of a day's log: something logged by hand, or a task ticked done.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    /// The note id or task id; unique across both within a day's list.
    pub id: String,
    pub when: DateTime<Local>,
    pub text: String,
    /// A completed task rather than a logged line. Tasks are managed on the
    /// Tasks page (untick there), so the Done page offers no delete for them.
    pub is_task: bool,
}

/// Everything that happened on `day`, oldest first: logged lines plus tasks
/// completed that day.
pub fn log_for_day(notes: &NoteStore, tasks: &TaskStore, day: NaiveDate) -> Vec<LogEntry> {
    let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Local));
    let logged = notes.accomplishments_on(day).into_iter().filter_map(|n| {
        Some(LogEntry { id: n.id.clone(), when: parse(&n.created)?, text: n.body.clone(), is_task: false })
    });
    let done = tasks.completed_on(day).into_iter().filter_map(|t| {
        Some(LogEntry {
            id: t.id.clone(),
            when: parse(t.completed.as_deref()?)?,
            text: t.text.clone(),
            is_task: true,
        })
    });
    let mut entries: Vec<LogEntry> = logged.chain(done).collect();
    entries.sort_by(|a, b| a.when.cmp(&b.when).then_with(|| a.id.cmp(&b.id)));
    entries
}

/// Every day with something in its log, newest first.
pub fn log_days(notes: &NoteStore, tasks: &TaskStore) -> Vec<NaiveDate> {
    let mut days = notes.accomplishment_days();
    days.extend(tasks.completed_days());
    days.sort_unstable_by(|a, b| b.cmp(a));
    days.dedup();
    days
}

/// A day's log as pasteable text: a dated heading and one bullet per line.
/// Multi-line entries are flattened so each stays one bullet.
pub fn summary<S: AsRef<str>>(day: NaiveDate, lines: &[S]) -> String {
    let mut out = format!("Done on {}", day.format("%A, %-d %B %Y"));
    for text in lines {
        let line = text.as_ref().split_whitespace().collect::<Vec<_>>().join(" ");
        out.push_str("\n- ");
        out.push_str(&line);
    }
    out
}

#[cfg(test)]
#[path = "accomplishments_tests.rs"]
mod tests;
