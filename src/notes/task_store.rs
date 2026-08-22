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
use super::task::{Task, TaskStatus};
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
}

impl Default for TaskStore {
    fn default() -> Self {
        Self { tasks: Vec::new(), path: Self::storage_path(), dirty: false }
    }
}

/// No caller until the suggestion chips land in the UI batch. Every method in
/// this block is in the same position, so the allow sits on the block rather
/// than being repeated per item.
impl TaskStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("tasks.json")
    }

    pub fn load() -> Self {
        Self::load_from(Self::storage_path())
    }

    /// The real load, parameterized by path so the corrupt-file branch can be
    /// exercised in a test.
    pub(crate) fn load_from(path: PathBuf) -> Self {
        if !path.exists() {
            return Self { tasks: Vec::new(), path, dirty: false };
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read tasks at {:?}: {}", path, e);
                return Self { tasks: Vec::new(), path, dirty: false };
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
                tracing::error!(
                    "Tasks at {:?} are not valid JSON ({}); preserving as {:?}",
                    path, e, backup
                );
                if let Err(e) = std::fs::rename(&path, &backup) {
                    tracing::error!("Could not preserve corrupt tasks: {}", e);
                }
                Self { tasks: Vec::new(), path, dirty: false }
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

    /// Build an undecided row. The model pass produces several of these in one
    /// instant, so the id comes from `notes::next_id` — a process-monotonic
    /// counter — rather than a timestamp, which would collide within the
    /// millisecond and give two chips the same identity.
    pub fn new_suggestion(
        note_id: &str,
        text: String,
        evidence: String,
        confidence: f32,
    ) -> Task {
        Task {
            id: next_id(),
            note_id: note_id.to_string(),
            text,
            evidence,
            confidence,
            status: TaskStatus::Suggested,
            done: false,
            created: Local::now().to_rfc3339(),
            decided: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PID-scoped temp dir, distinct from the notes tests' dir so the two
    /// cannot collide on a shared tag.
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("beamer_tasks_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Constructed directly rather than via `load()`, which reads a fixed path
    /// under the real config dir.
    fn temp_store(tag: &str) -> TaskStore {
        let path = temp_dir().join(format!("{tag}.json"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json.corrupt"));
        TaskStore { tasks: Vec::new(), path, dirty: false }
    }

    fn suggest(store: &mut TaskStore, note_id: &str, text: &str) -> String {
        let task = TaskStore::new_suggestion(note_id, text.into(), text.into(), 0.9);
        let id = task.id.clone();
        store.tasks.push(task);
        id
    }

    #[test]
    fn dismissed_rows_are_retained_with_a_decided_timestamp() {
        let mut store = temp_store("dismiss");
        let id = suggest(&mut store, "note-1", "Call the vet");

        assert!(store.dismiss(&id), "a fresh suggestion must accept a decision");

        let task = store.tasks.iter().find(|t| t.id == id).expect(
            "dismissing must never delete — the row is the labelled negative the \
             eval corpus is built from",
        );
        assert_eq!(task.status, TaskStatus::Dismissed);
        assert!(
            task.decided.is_some(),
            "a decided row with no decision time cannot be used as a labelled example"
        );
        assert!(
            !store.is_dirty() && store.path.exists(),
            "a decision must reach disk immediately, not wait for a debounce tick \
             that a crash could cost us"
        );
    }

    #[test]
    fn replace_suggestions_keeps_decided_rows() {
        let mut store = temp_store("replace");
        let decided = suggest(&mut store, "note-1", "Call the vet");
        let stale = suggest(&mut store, "note-1", "Buy milk maybe");
        let other = suggest(&mut store, "note-2", "Send the invoice");
        store.accept(&decided);

        let fresh = TaskStore::new_suggestion("note-1", "Book a table".into(), "book".into(), 0.7);
        let fresh_id = fresh.id.clone();
        store.replace_suggestions("note-1", vec![fresh]);

        assert!(
            store.tasks.iter().any(|t| t.id == decided && t.status == TaskStatus::Accepted),
            "re-running extraction must not destroy a decision the user already made"
        );
        assert!(
            !store.tasks.iter().any(|t| t.id == stale),
            "an undecided proposal for this note is superseded, not duplicated"
        );
        assert!(
            store.tasks.iter().any(|t| t.id == other),
            "another note's chips must be untouched — clearing them would delete \
             corpus rows for a note nobody re-ran"
        );
        assert!(store.tasks.iter().any(|t| t.id == fresh_id));
        assert_eq!(store.suggested_for("note-1").len(), 1);
    }

    #[test]
    fn a_corrupt_tasks_file_is_preserved_not_overwritten() {
        let path = temp_dir().join("corrupt.json");
        let backup = path.with_extension("json.corrupt");
        let _ = std::fs::remove_file(&backup);
        let garbage = "{ this is not json at all";
        std::fs::write(&path, garbage).unwrap();

        let store = TaskStore::load_from(path.clone());

        assert!(store.tasks.is_empty());
        assert!(!path.exists(), "the unreadable file is moved aside, not left to be overwritten");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            garbage,
            "the original bytes must survive verbatim — accepts and dismissals are \
             not re-derivable, so a lost tasks.json is lost eval signal"
        );
    }

    #[test]
    fn suggestions_made_in_the_same_millisecond_get_distinct_ids() {
        let a = TaskStore::new_suggestion("note-1", "one".into(), "one".into(), 0.5);
        let b = TaskStore::new_suggestion("note-1", "two".into(), "two".into(), 0.5);
        assert_ne!(
            a.id, b.id,
            "one extraction pass emits several tasks in a single instant; colliding \
             ids would make accept hit the wrong chip"
        );
    }

    #[test]
    fn a_redundant_decision_neither_writes_nor_moves_the_timestamp() {
        let mut store = temp_store("redundant");
        let id = suggest(&mut store, "note-1", "Call the vet");
        store.accept(&id);
        let first = store.tasks[0].decided.clone();

        assert!(!store.dismiss(&id), "accept and dismiss are terminal");
        assert_eq!(store.tasks[0].status, TaskStatus::Accepted);
        assert_eq!(
            store.tasks[0].decided, first,
            "re-deciding must not rewrite the decision time the corpus depends on"
        );
    }

    #[test]
    fn deciding_a_missing_id_does_not_flush_unrelated_changes() {
        let mut store = temp_store("missing");
        suggest(&mut store, "note-1", "Call the vet");
        store.dirty = true;

        assert!(!store.accept("no-such-task"));
        assert!(
            store.is_dirty() && !store.path.exists(),
            "a no-op decision must not trigger a write of whatever else happened to be pending"
        );
    }

    #[test]
    fn accepted_excludes_suggested_and_dismissed_rows() {
        let mut store = temp_store("accepted");
        let yes = suggest(&mut store, "note-1", "Call the vet");
        let no = suggest(&mut store, "note-1", "Learn the piano");
        suggest(&mut store, "note-1", "Undecided");
        store.accept(&yes);
        store.dismiss(&no);

        let accepted: Vec<&str> = store.accepted().iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            accepted,
            vec!["Call the vet"],
            "only confirmed rows are tasks; nothing enters a task list unconfirmed"
        );
        assert_eq!(store.suggested_for("note-1").len(), 1);
    }

    #[test]
    fn only_an_accepted_task_can_be_marked_done() {
        let mut store = temp_store("done");
        let pending = suggest(&mut store, "note-1", "Call the vet");
        store.flush_if_dirty();

        store.set_done(&pending, true);
        assert!(
            !store.tasks[0].done && !store.is_dirty(),
            "a suggestion is not a task yet, so it cannot be completed"
        );

        store.accept(&pending);
        store.set_done(&pending, true);
        assert!(store.tasks[0].done);
        assert!(store.is_dirty(), "a checkbox is debounced, not flushed inline");

        store.flush_if_dirty();
        store.set_done(&pending, true);
        assert!(!store.is_dirty(), "a redundant set_done must not schedule a write");
    }

    #[test]
    fn tasks_round_trip_through_disk() {
        let mut store = temp_store("roundtrip");
        let id = suggest(&mut store, "note-7", "Call the vet");
        store.tasks[0].evidence = "yeah I need to call the vet about Biscuit".into();
        store.tasks[0].confidence = 0.82;
        store.dismiss(&id);

        let reloaded = TaskStore::load_from(store.path.clone());

        assert_eq!(reloaded.tasks.len(), 1);
        assert_eq!(reloaded.tasks[0].note_id, "note-7", "provenance must survive a restart");
        assert_eq!(reloaded.tasks[0].evidence, "yeah I need to call the vet about Biscuit");
        assert_eq!(reloaded.tasks[0].confidence, 0.82);
        assert_eq!(reloaded.tasks[0].status, TaskStatus::Dismissed);
        assert!(reloaded.tasks[0].decided.is_some());
        assert!(!store.path.with_extension("json.tmp").exists());
    }

    #[test]
    fn confidence_is_stored_as_reported_not_clamped() {
        let out_of_range = TaskStore::new_suggestion("note-1", "x".into(), "x".into(), 1.7);
        assert_eq!(
            out_of_range.confidence, 1.7,
            "an impossible confidence means the prompt or the parser misfired, and \
             quietly flattening it hides the one thing the corpus should show"
        );
    }
}
