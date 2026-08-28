//! Sticky note storage.
//!
//! Follows `ui::history`'s load/save/corrupt-backup pattern, with two
//! deliberate differences: writes are debounced rather than per-change
//! (notes are edited per keystroke, and history's rewrite-everything-on-append
//! would be pathological here), and there is no entry cap — notes are authored
//! content, so they are archived rather than evicted.

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::config::Config;

pub mod blocks;
pub mod edit;
pub mod ics;
/// Public so `StageOutcome` is nameable from the model-pass callers; a private
/// module would make it a private-in-public return type.
pub mod lifecycle;
mod machine;
mod model;
pub mod pipeline;
pub mod task;
pub mod task_store;
pub use machine::MachineStore;
pub use model::{Attachment, Note, NoteColor, NoteOrigin, StageState};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteStore {
    pub notes: Vec<Note>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
    #[serde(skip)]
    pub(crate) dirty: bool,
    /// Window geometry and openness, keyed by note id. Never synced. See
    /// `machine::MachineStore`'s module doc for why it lives apart from
    /// `Note`. Skipped here too: `machine.json` is its own file, written
    /// through its own atomic save.
    #[serde(skip)]
    machine: MachineStore,
}

impl Default for NoteStore {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            path: Self::storage_path(),
            dirty: false,
            machine: MachineStore::new(Self::machine_storage_path()),
        }
    }
}

/// Monotonic within a process run, so two notes created in the same
/// millisecond still get distinct ids without pulling in a uuid dependency.
///
/// `pub(crate)` so attachment and task ids come from the same scheme.
/// Dropping three files at once, or extracting several tasks from one note,
/// must not give two of them the same id, which a timestamp alone would.
///
/// Note ids are minted by `next_note_id` instead, not this function. See its
/// doc comment for why they need a machine component and this scheme does not.
pub(crate) fn next_id() -> String {
    let (millis, n) = raw_id_parts();
    format!("{millis:x}-{n:04x}")
}

/// Same counter as `next_id`, plus a per-install suffix.
///
/// `Task.note_id` is a foreign key into the note id namespace. Two machines
/// creating their first note in the same millisecond both produce
/// `…-0000` under the plain scheme above, and a sync merge would then have
/// two machines' unrelated notes sharing one id, silently reparenting one
/// machine's tasks onto the other's note. The suffix is `machine`, this
/// install's `MachineStore::machine_id`, so that collision cannot happen
/// even at the same millisecond and the same counter value.
///
/// Sharing `raw_id_parts`' counter with `next_id` is deliberate, for the same
/// reason `next_id`'s doc comment gives for attachments: two notes created in
/// the same millisecond must not draw the same counter value either.
pub(crate) fn next_note_id(machine: &str) -> String {
    let (millis, n) = raw_id_parts();
    format_note_id(millis, n, machine)
}

fn raw_id_parts() -> (i64, u32) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let millis = Local::now().timestamp_millis();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    (millis, n)
}

fn format_note_id(millis: i64, counter: u32, machine: &str) -> String {
    format!("{millis:x}-{counter:04x}-{machine}")
}

/// Fields `notes.json` carried before this task, read independently of
/// `Note`'s own (now narrower) shape so a legacy file's window state can be
/// lifted into `machine.json` without losing anything. `Note` has no
/// `deny_unknown_fields`, so its own parse just ignores these keys; this is
/// the parse that catches them on the way past.
#[derive(Deserialize)]
struct LegacyWindowFields {
    id: String,
    #[serde(default)]
    pos: Option<(i32, i32)>,
    #[serde(default)]
    size: Option<(u32, u32)>,
    #[serde(default)]
    open: bool,
}

#[derive(Deserialize)]
struct LegacyNotesFile {
    #[serde(default)]
    notes: Vec<LegacyWindowFields>,
}

impl NoteStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("notes.json")
    }

    fn machine_storage_path() -> PathBuf {
        Config::config_dir().join("machine.json")
    }

    pub fn load() -> Self {
        Self::load_from(Self::storage_path(), Self::machine_storage_path())
    }

    /// The real logic behind `load()`, taking both paths explicitly so it is
    /// testable without reaching into the user's real config dir, the same
    /// improvement `TaskStore::load_from` made over the equivalent code here
    /// before it existed.
    fn load_from(path: PathBuf, machine_path: PathBuf) -> Self {
        let mut machine = MachineStore::load_from(machine_path);

        if !path.exists() {
            machine.gc(&std::collections::HashSet::new());
            return Self { notes: Vec::new(), path, dirty: false, machine };
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read notes at {:?}: {}", path, e);
                machine.gc(&std::collections::HashSet::new());
                return Self { notes: Vec::new(), path, dirty: false, machine };
            }
        };
        match serde_json::from_str::<NoteStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store.dirty = false;
                store.machine = machine;
                store.migrate_legacy_window_state(&contents);
                let valid: std::collections::HashSet<&str> =
                    store.notes.iter().map(|n| n.id.as_str()).collect();
                store.machine.gc(&valid);
                store
            }
            Err(e) => {
                // Same reasoning as history.rs: don't start empty, or the next
                // write destroys the user's notes for good.
                let backup = path.with_extension("json.corrupt");
                tracing::error!(
                    "Notes at {:?} are not valid JSON ({}); preserving as {:?}",
                    path, e, backup
                );
                let _ = std::fs::rename(&path, &backup);
                machine.gc(&std::collections::HashSet::new());
                Self { notes: Vec::new(), path, dirty: false, machine }
            }
        }
    }

    /// Lift `pos`/`size`/`open` off a `notes.json` written before this task,
    /// into `machine.json`. A no-op once every note in the file has been
    /// migrated once (`MachineStore::migrate_legacy` will not overwrite an
    /// existing entry), and a no-op forever after the first save, since
    /// `Note` stops serializing these fields at all.
    fn migrate_legacy_window_state(&mut self, contents: &str) {
        let Ok(legacy) = serde_json::from_str::<LegacyNotesFile>(contents) else {
            return;
        };
        for note in legacy.notes {
            self.machine.migrate_legacy(&note.id, note.pos, note.size, note.open);
        }
    }

    /// Persist the store, replacing the file atomically.
    ///
    /// Writes to a sibling temp file and renames over the target, so a crash
    /// mid-write leaves the previous notes intact rather than a half-written
    /// file that `load()` would then quarantine.
    ///
    /// The directory created is `self.path`'s parent, not `Config::config_dir()`
    /// as in `history.rs` — the two are the same in production, but the tests
    /// point `path` at a temp dir and must not reach into the real config dir.
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

    /// Write only if something changed since the last flush. Driven by a
    /// ~500ms interval task so per-keystroke edits coalesce into one write.
    ///
    /// Covers both files. `notes.json` and `machine.json` fail independently:
    /// if one write errors, its store stays dirty for the next tick to retry
    /// while the other still lands.
    pub fn flush_if_dirty(&mut self) -> bool {
        let mut wrote = false;
        if self.dirty {
            match self.save() {
                Ok(()) => {
                    self.dirty = false;
                    wrote = true;
                }
                Err(e) => tracing::error!("Failed to save notes: {}", e),
            }
        }
        if self.machine.flush_if_dirty() {
            wrote = true;
        }
        wrote
    }

    /// Whether either store has an edit pending a write. Read before taking a
    /// `write()` lock on the signal so an idle tick does not notify every
    /// subscriber.
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.machine.is_dirty()
    }

    /// `origin` is passed explicitly rather than defaulted: it is corpus
    /// provenance, and a silent default is exactly what corrupts a corpus.
    pub fn create(&mut self, raw: String, color: NoteColor, origin: NoteOrigin) -> String {
        let now = Local::now().to_rfc3339();
        let id = next_note_id(&self.machine.machine_id);
        self.notes.push(Note {
            id: id.clone(),
            created: now.clone(),
            modified: now,
            body: raw.clone(),
            raw,
            clean_state: StageState::Pending,
            extract_state: StageState::Pending,
            origin,
            color,
            attachments: Vec::new(),
            archived: false,
        });
        self.machine.set_open(&id, true);
        self.dirty = true;
        id
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// Where this note's window last sat, in logical coordinates. Machine-
    /// local, see `machine::MachineStore`.
    ///
    /// No production caller yet, matching `Note::pos`'s status before this
    /// task. Nothing captures a window's actual position on Linux, and
    /// nothing should (see `set_pos`). Kept, and given a `NoteStore` method
    /// alongside `MachineStore`'s, because it is part of the persisted schema
    /// and `with_position` is honoured natively on Windows. A future
    /// placement feature reads it from here.
    #[allow(dead_code)]
    pub fn pos(&self, id: &str) -> Option<(i32, i32)> {
        self.machine.pos(id)
    }

    /// This note's window size, in logical pixels. Machine-local.
    pub fn size(&self, id: &str) -> Option<(u32, u32)> {
        self.machine.size(id)
    }

    /// Whether this note's window is showing. Machine-local.
    pub fn is_open(&self, id: &str) -> bool {
        self.machine.is_open(id)
    }

    /// Record where this note's window last sat. A machine write like
    /// `set_size`: does not bump `modified`, does not dirty `notes.json`.
    ///
    /// No production caller yet. See `pos`'s doc comment.
    #[allow(dead_code)]
    pub fn set_pos(&mut self, id: &str, pos: (i32, i32)) {
        if self.get(id).is_none() {
            return;
        }
        self.machine.set_pos(id, pos);
    }

    fn touch(&mut self, id: &str) -> Option<&mut Note> {
        let now = Local::now().to_rfc3339();
        let note = self.notes.iter_mut().find(|n| n.id == id)?;
        note.modified = now;
        Some(note)
    }

    pub fn set_body(&mut self, id: &str, body: String) {
        if let Some(note) = self.touch(id) {
            note.body = body;
            self.dirty = true;
        }
    }

    pub fn set_color(&mut self, id: &str, color: NoteColor) {
        if let Some(note) = self.touch(id) {
            note.color = color;
            self.dirty = true;
        }
    }

    /// Record whether a note's window is showing.
    ///
    /// Machine-local and does **not** call `touch()`. It used to. Bumping
    /// `modified` here is what made `open` dangerous under any last-write-
    /// wins sync merge. Closing a sticky on one machine would make that note
    /// look newer than a real edit made on another and win a merge it had no
    /// business winning. `MachineStore::set_open` still no-ops when the value
    /// is unchanged, so a redundant call from an event handler costs nothing.
    pub fn set_open(&mut self, id: &str, open: bool) {
        if self.get(id).is_none() {
            return;
        }
        self.machine.set_open(id, open);
    }

    pub fn archive(&mut self, id: &str) {
        let Some(note) = self.touch(id) else { return };
        note.archived = true;
        self.dirty = true;
        self.machine.set_open(id, false);
    }

    /// Return an archived note to the active list.
    ///
    /// Leaves `open` alone. Restoring puts a note back on the board; popping a
    /// window open on top of that would be a second, unasked-for action, and
    /// clicking the card is already how you get the window back.
    pub fn restore(&mut self, id: &str) {
        if self.get(id).is_none_or(|n| !n.archived) {
            return;
        }
        if let Some(note) = self.touch(id) {
            note.archived = false;
            self.dirty = true;
        }
    }

    /// Non-archived notes, newest first.
    pub fn active(&self) -> Vec<&Note> {
        Self::newest_first(self.notes.iter().filter(|n| !n.archived).collect())
    }

    /// Archived notes, newest first.
    pub fn archived(&self) -> Vec<&Note> {
        Self::newest_first(self.notes.iter().filter(|n| n.archived).collect())
    }

    /// Active notes matching `query`, newest first. An empty query matches all.
    ///
    /// Searches `raw` as well as `body`. A cleanup pass rewrites `body` and can
    /// remove the very words that were spoken, so searching only the display
    /// text would fail to find a note by something you actually said — which is
    /// the most natural thing to search for.
    ///
    /// `body` is matched through `blocks::plain_text`, so a note's own
    /// attachment tokens are not searchable text. Without that, every note
    /// holding an image would match the query "beamer".
    pub fn search(&self, query: &str) -> Vec<&Note> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self.active();
        }
        Self::newest_first(
            self.notes
                .iter()
                .filter(|n| !n.archived)
                .filter(|n| {
                    blocks::plain_text(&n.body).to_lowercase().contains(&needle)
                        || n.raw.to_lowercase().contains(&needle)
                })
                .collect(),
        )
    }

    fn newest_first(mut v: Vec<&Note>) -> Vec<&Note> {
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
