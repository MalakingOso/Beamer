//! Persistence for extracted tasks.
//!
//! A separate file from `notes.json` rather than a `tasks` field on `Note`.
//! The two have different write rhythms — a note is dirtied per keystroke and
//! debounced, a decision is written the instant it is made — and tasks outlive
//! their notes as corpus rows regardless of what happens to the note.
//!
//! Load/save/quarantine follows `ui::history` and `NoteStore`. The one
//! deliberate improvement over `NoteStore::load` is that the parse-and-
//! quarantine step lives in `load_from(path)`, so it is testable without
//! reaching into the user's real config dir. `NoteStore::load` reads a fixed
//! path and its corrupt branch is consequently untested; don't copy that.

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::next_id;
use super::sync_doc::SyncHandle;
use super::task::{Proposal, Task, TaskStatus};
use super::{doc_tasks, NoteStore};
use crate::config::Config;

/// No caller until the suggestion chips land in the UI batch; the store is
/// built and tested first so the extraction pass has somewhere to write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStore {
    pub tasks: Vec<Task>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
    #[serde(skip)]
    pub(crate) dirty: bool,
    /// The same automerge document `NoteStore` holds. Notes and tasks are one
    /// corpus on disk even though they are two stores in memory.
    #[serde(skip)]
    pub(crate) doc: SyncHandle,
    /// A decision that has reached `tasks.json` but not yet the document. See
    /// `NoteStore::doc_dirty`.
    #[serde(skip)]
    pub(crate) doc_dirty: bool,
    /// Why the corpus failed to load, if it did. Surfaced to the status log
    /// by `App()`.
    #[serde(skip)]
    pub load_error: Option<String>,
    /// Document entries that could not be read back into a `Task`. Kept so
    /// the next reconcile does not prune them.
    #[serde(skip)]
    pub(crate) unreadable_tasks: Vec<String>,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            path: Self::storage_path(),
            dirty: false,
            doc: SyncHandle::default(),
            doc_dirty: false,
            load_error: None,
            unreadable_tasks: Vec::new(),
        }
    }
}

/// No caller until the suggestion chips land in the UI batch. Every method in
/// this block is in the same position, so the allow sits on the block rather
/// than being repeated per item.
impl TaskStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("tasks.json")
    }

    /// Load beside an already-loaded `NoteStore`, sharing its document.
    ///
    /// The note store is loaded first because it owns the document handle and
    /// the machine id the document's actor is derived from. Where the
    /// document already existed, the tasks come out of it and `tasks.json` is
    /// a derived export from then on; otherwise the existing JSON seeds it,
    /// once.
    pub fn load_beside(notes: &NoteStore) -> Self {
        Self::load_beside_at(notes, Self::storage_path())
    }

    /// `load_beside`, with the mirror path injected so a test never reaches
    /// into the real config dir.
    pub(crate) fn load_beside_at(notes: &NoteStore, path: PathBuf) -> Self {
        let doc = notes.sync_doc();
        let from_document = doc.lock().existed();
        if from_document {
            let hydrated = doc_tasks::hydrate(&doc.lock());
            let load_error = (!hydrated.unreadable.is_empty()).then(|| {
                format!(
                    "{} rows in the tasks document could not be read and are being left \
                     alone; they are not on the Tasks page",
                    hydrated.unreadable.len()
                )
            });
            return Self {
                tasks: hydrated.tasks,
                path,
                dirty: false,
                doc,
                doc_dirty: false,
                load_error,
                unreadable_tasks: hydrated.unreadable,
            };
        }
        let mut store = Self::load_from(path);
        store.doc = doc;
        store.doc_dirty = true;
        store
    }

    /// The real load, parameterized by path so the corrupt-file branch can be
    /// exercised in a test. Only ever the seed path now: once the document
    /// exists, `load_beside` reads that instead.
    pub(crate) fn load_from(path: PathBuf) -> Self {
        let empty = |path: PathBuf| Self { tasks: Vec::new(), path, ..Self::detached() };
        if !path.exists() {
            return empty(path);
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                let message = format!("Could not read tasks at {}: {e}", path.display());
                tracing::error!("{message}");
                let mut store = empty(path);
                store.load_error = Some(message);
                return store;
            }
        };
        match serde_json::from_str::<TaskStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store.dirty = false;
                store
            }
            Err(e) => {
                // Same reasoning as history.rs: starting empty would let the
                // next write overwrite the file for good. Here that would cost
                // more than notes — every accept and dismiss the user has ever
                // made is a labelled example, and they are not re-derivable.
                let backup = path.with_extension("json.corrupt");
                let message = format!(
                    "Tasks at {} are not valid JSON ({e}); preserved as {}",
                    path.display(),
                    backup.display()
                );
                tracing::error!("{message}");
                if let Err(e) = std::fs::rename(&path, &backup) {
                    tracing::error!("Could not preserve corrupt tasks: {}", e);
                }
                let mut store = empty(path);
                store.load_error = Some(message);
                store
            }
        }
    }

    /// Persist the store, replacing the file atomically.
    ///
    /// Temp file plus rename, so a crash mid-write leaves the previous tasks
    /// intact rather than a half-written file `load()` would then quarantine.
    /// The directory created is `self.path`'s parent, not `Config::config_dir()`
    /// — the same in production, but the tests point `path` at a temp dir.
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let contents = serde_json::to_string_pretty(self)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, contents)?;
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Write only if something changed. Driven by the same interval task that
    /// flushes notes, so a run of `set_done` toggles coalesces into one write.
    pub fn flush_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        // The document write belongs to the 500 ms tick. See
        // `NoteStore::flush_if_dirty`.
        self.doc_dirty = true;
        if let Err(e) = self.save() {
            tracing::error!("Failed to save tasks: {}", e);
            // Stay dirty so the next tick retries rather than losing the row.
            return false;
        }
        self.dirty = false;
        true
    }

    /// Whether a change is pending a write.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether either the JSON mirror or the document still owes a write.
    pub fn needs_flush(&self) -> bool {
        self.is_dirty() || self.doc_dirty
    }

    /// An empty store with no document behind it, for the load paths that
    /// have not been handed a handle yet and for tests that never want a
    /// file. Saving a detached document is a no-op.
    pub(crate) fn detached() -> Self {
        Self {
            tasks: Vec::new(),
            path: PathBuf::new(),
            dirty: false,
            doc: SyncHandle::default(),
            doc_dirty: false,
            load_error: None,
            unreadable_tasks: Vec::new(),
        }
    }

    /// Install a fresh set of proposals for one note, keeping every decision.
    ///
    /// Re-running extraction must neither duplicate chips nor destroy an answer
    /// the user already gave. Only `Suggested` rows for `note_id` are cleared;
    /// an `Accepted` or `Dismissed` row survives untouched, and rows belonging
    /// to other notes are never considered. The naive
    /// `retain(|t| t.note_id != note_id)` gets this wrong and silently deletes
    /// corpus rows — the deletion is invisible because the re-run immediately
    /// repopulates the chips with something that looks right.
    pub fn replace_suggestions(&mut self, note_id: &str, proposals: Vec<Task>) {
        debug_assert!(
            proposals.iter().all(|t| t.note_id == note_id),
            "a proposal carrying a different note_id would appear on another \
             note's chips and mislabel that note in the corpus"
        );
        let before = self.tasks.len();
        self.tasks.retain(|t| t.note_id != note_id || t.status != TaskStatus::Suggested);
        let removed = before != self.tasks.len();
        if !removed && proposals.is_empty() {
            return;
        }
        self.tasks.extend(proposals);
        self.dirty = true;
    }

    /// Confirm a proposal. Terminal: an accepted row is never re-proposed.
    ///
    /// Returns whether a decision was recorded — not whether the write
    /// succeeded. A failed `save()` leaves the store dirty and the next flush
    /// retries, which is `flush_if_dirty`'s existing contract; the caller wants
    /// to know whether the user's click landed, not whether the disk cooperated.
    pub fn accept(&mut self, id: &str) -> bool {
        self.decide(id, TaskStatus::Accepted)
    }

    /// Reject a proposal. Terminal, and **non-destructive**.
    ///
    /// A dismissed row is retained forever. It is the labelled negative that
    /// makes the eval corpus grow every time the feature is used, and negatives
    /// are the scarce half — precision is what extraction is judged on, so
    /// "the model proposed this and the user said no" is the single most
    /// valuable record the app produces.
    ///
    /// A future reader will look at a table full of dismissed rows and see junk
    /// to clean up. There is no cleanup to do: deleting them does not free
    /// anything worth freeing and it destroys data that cannot be regenerated,
    /// because it only exists as a record of a decision a human made once. If
    /// they are ever in the way, hide them in the UI.
    pub fn dismiss(&mut self, id: &str) -> bool {
        self.decide(id, TaskStatus::Dismissed)
    }

    /// Stamp a terminal status and flush immediately.
    ///
    /// Inline rather than waiting for the debounce tick, on the same argument
    /// that makes `do_note_capture` flush inline: a lost keystroke is
    /// retypeable, a lost decision is lost eval signal.
    ///
    /// Deciding an already-decided row, or a missing id, is a no-op that does
    /// not dirty the store and does not flush — matching `NoteStore::set_open`.
    /// Gating the flush on the guard matters: an unconditional flush here would
    /// write out unrelated pending changes on a call that did nothing.
    fn decide(&mut self, id: &str, status: TaskStatus) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        if task.status != TaskStatus::Suggested {
            return false;
        }
        task.status = status;
        // Written with the status, never after it. A decided row with no
        // decision time cannot be used as a labelled example.
        task.decided = Some(Local::now().to_rfc3339());
        self.dirty = true;
        self.flush_if_dirty();
        true
    }

    /// Tick or untick an accepted task.
    ///
    /// Only accepted rows can be done — a suggestion is not a task yet, and a
    /// dismissed one never became one. Debounced rather than flushed inline:
    /// unlike a decision, a checkbox is trivially re-tickable and carries no
    /// eval signal, so it does not earn a synchronous write.
    pub fn set_done(&mut self, id: &str, done: bool) {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if task.status != TaskStatus::Accepted || task.done == done {
            return;
        }
        task.done = done;
        self.dirty = true;
    }

    /// Undecided proposals for one note — the chips its window shows.
    pub fn suggested_for(&self, note_id: &str) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.note_id == note_id && t.status == TaskStatus::Suggested)
            .collect()
    }

    /// Confirmed tasks across every note, newest first.
    pub fn accepted(&self) -> Vec<&Task> {
        let mut v: Vec<&Task> =
            self.tasks.iter().filter(|t| t.status == TaskStatus::Accepted).collect();
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }

    /// Set a due date by hand — the picker beside an unresolved phrase.
    ///
    /// The user answering a question the model could not is a decision worth
    /// keeping, so this flushes inline for the same reason `decide` does.
    /// `due_phrase` is deliberately **left alone**: it records what the model
    /// saw, and overwriting it would erase the evidence that it saw something
    /// it could not resolve — exactly the signal the eval corpus wants.
    pub fn set_due(&mut self, id: &str, due: Option<String>, all_day: bool) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        if task.due == due && task.due_all_day == all_day {
            return false;
        }
        task.due = due;
        task.due_all_day = all_day;
        self.dirty = true;
        self.flush_if_dirty();
        true
    }

    /// Drop every row belonging to a note that was deleted outright.
    ///
    /// **A deliberate exception to "dismissed rows are retained as labelled
    /// negatives".** Everywhere else in this store a decision is permanent
    /// corpus data and nothing may remove it. Here the user has explicitly
    /// deleted the note those rows describe, behind a confirm, and an explicit
    /// delete means gone — keeping the rows would leave the corpus holding
    /// labels for a note whose text no longer exists to explain them.
    ///
    /// Flushed inline: the note's own deletion is written at the same moment,
    /// and leaving the two out of step across a crash would strand rows whose
    /// note is already gone.
    pub fn delete_for_note(&mut self, note_id: &str) -> usize {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.note_id != note_id);
        let removed = before - self.tasks.len();
        if removed > 0 {
            self.dirty = true;
            self.flush_if_dirty();
        }
        removed
    }

    /// Build an undecided row. The model pass produces several of these in one
    /// instant, so the id comes from `notes::next_id` — a process-monotonic
    /// counter — rather than a timestamp, which would collide within the
    /// millisecond and give two chips the same identity.
    ///
    /// Takes a whole [`Proposal`] rather than loose fields so a row cannot be
    /// written half-dated: setting `due` after construction is the kind of step
    /// that gets forgotten at one call site and produces a task whose chip says
    /// nothing and whose export is empty.
    pub fn new_suggestion(note_id: &str, proposal: Proposal) -> Task {
        Task {
            id: next_id(),
            note_id: note_id.to_string(),
            text: proposal.text,
            evidence: proposal.evidence,
            confidence: proposal.confidence,
            status: TaskStatus::Suggested,
            done: false,
            created: Local::now().to_rfc3339(),
            decided: None,
            due: proposal.due,
            due_all_day: proposal.due_all_day,
            due_phrase: proposal.due_phrase,
            kind: proposal.kind,
        }
    }
}

#[cfg(test)]
#[path = "task_store/tests.rs"]
mod tests;
