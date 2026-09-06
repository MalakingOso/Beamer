//! Persistence for extracted tasks. A separate file from `notes.json`: notes
//! dirty per keystroke and debounce, decisions write instantly, and tasks
//! outlive their notes as corpus rows.

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::next_synced_id;
use super::sync_doc::SyncHandle;
use super::task::{Proposal, Task, TaskStatus};
use super::{doc_tasks, NoteStore};
use crate::config::Config;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStore {
    pub tasks: Vec<Task>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
    #[serde(skip)]
    pub(crate) dirty: bool,
    /// The same automerge document `NoteStore` holds: one corpus on disk, two stores in memory.
    #[serde(skip)]
    pub(crate) doc: SyncHandle,
    /// Reached `tasks.json` but not yet the document.
    #[serde(skip)]
    pub(crate) doc_dirty: bool,
    #[serde(skip)]
    pub load_error: Option<String>,
    /// Document entries that could not be read back; kept so reconcile won't prune them.
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

impl TaskStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("tasks.json")
    }

    /// Load beside an already-loaded `NoteStore`, sharing its document. An
    /// existing document is the source; otherwise the JSON seeds it, once.
    pub fn load_beside(notes: &NoteStore) -> Self {
        Self::load_beside_at(notes, Self::storage_path())
    }

    /// `load_beside` with the mirror path injected, so tests avoid the real config dir.
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

    /// The real load, path-injected for tests. Only the seed path: once the
    /// document exists, `load_beside` reads that instead.
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
                // Quarantine first: accepts/dismisses are irreplaceable eval labels.
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

    /// Persist the store atomically (temp file plus rename).
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let contents = serde_json::to_string_pretty(self)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, contents)?;
        if let Err(e) = super::sync_doc::rename_with_retry(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Write only if something changed. A run of toggles coalesces into one write.
    pub fn flush_if_dirty(&mut self) -> bool {
        if self.doc.lock().is_read_only() {
            // Came up empty from an unreadable document; rewriting the mirror
            // would destroy every accept/dismiss on record.
            self.dirty = false;
            self.doc_dirty = false;
            return false;
        }
        if !self.dirty {
            return false;
        }
        self.doc_dirty = true; // the document write belongs to the 500 ms tick
        if let Err(e) = self.save() {
            tracing::error!("Failed to save tasks: {}", e);
            return false; // stay dirty so the next tick retries
        }
        self.dirty = false;
        true
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the JSON mirror or the document still owes a write.
    pub fn needs_flush(&self) -> bool {
        self.is_dirty() || self.doc_dirty
    }

    /// An empty store with no document behind it, for load paths and file-less tests.
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
    /// Only `Suggested` rows for `note_id` are cleared; decided rows survive,
    /// or re-runs would silently delete corpus labels behind fresh-looking chips.
    pub fn replace_suggestions(&mut self, note_id: &str, proposals: Vec<Task>) {
        debug_assert!(
            proposals.iter().all(|t| t.note_id == note_id),
            "a stray note_id would chip on the wrong note and mislabel the corpus"
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
    /// Returns whether the click landed, not whether the disk cooperated.
    pub fn accept(&mut self, id: &str) -> bool {
        self.decide(id, TaskStatus::Accepted)
    }

    /// Reject a proposal. Terminal and non-destructive: dismissed rows are
    /// retained as labelled negatives for the eval corpus. Never delete them
    /// to "clean up"; hide them in the UI instead.
    pub fn dismiss(&mut self, id: &str) -> bool {
        self.decide(id, TaskStatus::Dismissed)
    }

    /// Stamp a terminal status and flush inline: a lost decision is lost eval
    /// signal. A no-op on missing ids or already-decided rows, without flushing.
    fn decide(&mut self, id: &str, status: TaskStatus) -> bool {
        let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        if task.status != TaskStatus::Suggested {
            return false;
        }
        task.status = status;
        // Stamped with the status: a decided row without a decision time is no labelled example.
        task.decided = Some(Local::now().to_rfc3339());
        self.dirty = true;
        self.flush_if_dirty();
        true
    }

    /// Tick or untick an accepted task. Debounced, not inline: a checkbox is
    /// re-tickable and carries no eval signal.
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

    /// Undecided proposals for one note: the chips its window shows.
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

    /// Set a due date by hand; flushes inline like `decide`. `due_phrase` is
    /// left alone: it records what the model saw but could not resolve.
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

    /// Drop every row of an outright-deleted note — the one exception to
    /// retaining decisions, since labels for a gone note explain nothing.
    /// Flushed inline with the note's own deletion so a crash can't strand rows.
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

    /// Build an undecided row. The id comes from `notes::next_synced_id`,
    /// not a timestamp, since one pass mints several rows in the same
    /// millisecond — and carries the machine suffix, since rows sync keyed
    /// by id. Takes a whole [`Proposal`] so a row can't be written half-dated.
    pub fn new_suggestion(note_id: &str, proposal: Proposal, machine_id: &str) -> Task {
        Task {
            id: next_synced_id(machine_id),
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
