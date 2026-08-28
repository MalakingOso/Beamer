//! Sticky note storage.
//!
//! The `Vec<Note>` here is the in-memory source of truth. What it persists to
//! is an automerge document shared with `TaskStore`, at
//! `<config_dir>/sync/notes.automerge`, so two machines editing offline merge
//! instead of one overwriting the other. `notes.json` survives as a derived
//! export, written but never read; `task_eval` and a curious pair of eyes are
//! its readers. See `sync_doc` for the document, `flush` for the one place it
//! is written, and `legacy` for the one-time seed off the old JSON-only store.
//!
//! Writes are debounced rather than per-change (notes are edited per
//! keystroke, and history's rewrite-everything-on-append would be pathological
//! here), and there is no entry cap: notes are authored content, so they are
//! archived rather than evicted.

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::config::Config;
use sync_doc::{SyncDoc, SyncHandle};

pub mod blocks;
mod doc_notes;
mod doc_tasks;
pub mod edit;
pub mod flush;
pub mod ics;
mod legacy;
/// Public so `StageOutcome` is nameable from the model-pass callers; a private
/// module would make it a private-in-public return type.
pub mod lifecycle;
mod machine;
mod model;
pub mod pipeline;
pub mod sync_doc;
pub mod task;
pub mod task_store;
pub use flush::flush_stores;
pub use machine::MachineStore;
pub use model::{Attachment, Location, Note, NoteColor, NoteOrigin, StageState};

/// `<config_dir>/sync`, the root of everything that syncs between machines.
///
/// Takes `config_dir` explicitly rather than calling `Config::config_dir()`
/// itself, so it stays a pure function: a test can point it at a temp
/// directory instead of the user's real `~/.config/Beamer`. Tasks 9-11 build
/// the rest of `sync/` on top of this.
pub fn sync_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("sync")
}

/// Where Beamer keeps its own, content-addressed copy of attachment bytes.
/// See `model::Attachment`'s doc comment for why a note owns this copy
/// instead of pointing at wherever the user's original file happens to sit.
pub fn attachments_dir(config_dir: &Path) -> PathBuf {
    sync_dir(config_dir).join("attachments")
}

/// The automerge document holding both the note and task corpus.
///
/// One file, two roots. Tasks 10 and 11 share and sync exactly this path, so
/// splitting tasks into a second document would give them a second stream to
/// carry for no gain in merge behaviour.
pub fn notes_document_path(config_dir: &Path) -> PathBuf {
    sync_dir(config_dir).join("notes.automerge")
}

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
    /// Where this store's own copies of attachment bytes live. Injectable
    /// like `path` and `machine`'s path, so tests point it at a temp
    /// directory rather than the user's real `~/.config/Beamer/sync`.
    #[serde(skip)]
    pub(crate) attachments_dir: PathBuf,
    /// The automerge document, shared with `TaskStore`. See `sync_doc`.
    #[serde(skip)]
    pub(crate) doc: SyncHandle,
    /// An edit that has reached `notes.json` but not yet the document.
    ///
    /// `flush_if_dirty` clears `dirty` the moment it writes the JSON mirror,
    /// including at the call sites that flush inline. Without a second flag
    /// the 500 ms tick would see a clean store and the document would never
    /// catch up.
    #[serde(skip)]
    pub(crate) doc_dirty: bool,
    /// Why the corpus failed to load, if it did. `App()` pushes this into the
    /// status log on the first render. A corrupt store replaced by an empty
    /// one, with nothing said, is the failure this exists to stop.
    #[serde(skip)]
    pub load_error: Option<String>,
    /// Document entries that could not be read back into a `Note`. Kept so
    /// the next reconcile does not prune them and `machine.gc` does not wipe
    /// their window state. See `doc_notes::Hydrated`.
    #[serde(skip)]
    pub(crate) unreadable_notes: Vec<String>,
}

impl Default for NoteStore {
    fn default() -> Self {
        Self {
            notes: Vec::new(),
            path: Self::storage_path(),
            dirty: false,
            machine: MachineStore::new(Self::machine_storage_path()),
            attachments_dir: Self::attachments_storage_dir(),
            doc: SyncHandle::default(),
            doc_dirty: false,
            load_error: None,
            unreadable_notes: Vec::new(),
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

impl NoteStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("notes.json")
    }

    fn machine_storage_path() -> PathBuf {
        Config::config_dir().join("machine.json")
    }

    fn attachments_storage_dir() -> PathBuf {
        attachments_dir(&Config::config_dir())
    }

    fn document_storage_path() -> PathBuf {
        notes_document_path(&Config::config_dir())
    }

    pub fn load() -> Self {
        Self::load_from(
            Self::storage_path(),
            Self::machine_storage_path(),
            Self::attachments_storage_dir(),
            Self::document_storage_path(),
        )
    }

    /// The shared document handle, so `TaskStore` can hold the other half of
    /// the same corpus.
    pub fn sync_doc(&self) -> SyncHandle {
        self.doc.clone()
    }

    /// Whether anything is waiting on either the JSON mirror or the document.
    ///
    /// The tick reads this rather than `is_dirty`: an inline `flush_if_dirty`
    /// clears `dirty` as soon as the mirror lands, and the document write is
    /// still outstanding at that point.
    pub fn needs_flush(&self) -> bool {
        self.is_dirty() || self.doc_dirty
    }

    /// Whether the document file has been written since we last wrote it,
    /// which is how a copy synced in from another machine gets noticed. A
    /// stat, cheap enough for the 500 ms tick.
    pub fn doc_file_moved(&self) -> bool {
        self.doc.lock().file_moved()
    }

    /// The real logic behind `load()`, taking all four paths explicitly so
    /// it is testable without reaching into the user's real config dir, the
    /// same improvement `TaskStore::load_from` made over the equivalent code
    /// here before it existed.
    ///
    /// GC only runs in the `Ok` branch below, deliberately. A missing,
    /// unreadable or quarantined `notes.json` tells us nothing about which
    /// notes exist; it is a read failure, not proof of absence. GCing on any
    /// of those would permanently wipe `machine.json` on a transient error, a
    /// risk that stops being theoretical once a sync writer can be
    /// mid-replace of `notes.json` when this reads it. A genuine fresh
    /// install pays nothing for the restriction: its `machine.json` is
    /// already empty.
    fn load_from(
        path: PathBuf,
        machine_path: PathBuf,
        attachments_dir: PathBuf,
        doc_path: PathBuf,
    ) -> Self {
        let machine = MachineStore::load_from(machine_path);
        let (doc, load_error) = SyncDoc::open(doc_path);
        // A document already on disk is the corpus. `notes.json` is a derived
        // export from that point on, never read again, so a stale or
        // hand-edited mirror cannot resurrect anything.
        let from_document = doc.existed();
        let handle = SyncHandle::new(doc);

        if from_document {
            let hydrated = doc_notes::hydrate(&handle.lock());
            let unreadable_message = (!hydrated.unreadable.is_empty()).then(|| {
                format!(
                    "{} entries in the notes document could not be read and are being left \
                     alone; they are not on the board",
                    hydrated.unreadable.len()
                )
            });
            let mut store = Self {
                notes: hydrated.notes,
                path,
                dirty: false,
                machine,
                attachments_dir,
                doc: handle,
                doc_dirty: false,
                load_error: load_error.or(unreadable_message),
                unreadable_notes: hydrated.unreadable,
            };
            store.migrate_legacy_attachments();
            // An entry we could not read is a read failure, not proof the
            // note is gone, so its window state is spared along with its key.
            let valid: std::collections::HashSet<&str> = store
                .notes
                .iter()
                .map(|n| n.id.as_str())
                .chain(store.unreadable_notes.iter().map(String::as_str))
                .collect();
            store.machine.gc(&valid);
            return store;
        }

        let mut store = Self::seed_from_json(path, machine, attachments_dir, handle);
        // Seeding happens at most once per install. A second machine gets the
        // document through sync, never by seeding its own copy of the JSON:
        // two independent seeds mint different automerge object ids for the
        // same notes, and those do not merge character by character, they
        // conflict whole.
        store.doc_dirty = true;
        store.load_error = load_error.or_else(|| store.load_error.take());
        store
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
            // The document write belongs to the 500 ms tick, which is the one
            // place both stores have reconciled before a merge can land. All
            // this call can do is remember that the document still owes a
            // write.
            self.doc_dirty = true;
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

#[cfg(test)]
#[path = "sync_tests.rs"]
mod sync_tests;
