//! Sticky note storage. The `Vec<Note>` is the in-memory source of truth,
//! persisted to an automerge document shared with `TaskStore`; `notes.json` is
//! a derived export, written but never read. Writes are debounced; notes are
//! archived rather than evicted, so there is no entry cap.

use anyhow::Result;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::config::Config;
use sync_doc::{SyncDoc, SyncHandle};

pub mod blocks;
mod doc_notes;
mod doc_vocab;
mod doc_tasks;
pub mod edit;
pub mod flush;
pub mod ics;
mod legacy;
/// Public so `StageOutcome` is nameable from the model-pass callers.
pub mod lifecycle;
mod machine;
mod model;
pub mod pipeline;
pub mod sync_client;
pub mod sync_doc;
pub mod task;
pub mod task_store;
pub use flush::flush_stores;
pub use machine::MachineStore;
pub use model::{Attachment, Location, Note, NoteColor, NoteOrigin, StageState};

/// `<config_dir>/sync`, the root of everything that syncs. Takes `config_dir`
/// explicitly so tests can point it at a temp directory.
pub fn sync_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("sync")
}

/// Where Beamer keeps its own content-addressed copy of attachment bytes.
pub fn attachments_dir(config_dir: &Path) -> PathBuf {
    sync_dir(config_dir).join("attachments")
}

/// The automerge document holding both the note and task corpus. One file, two roots.
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
    /// Window geometry and openness, keyed by note id. Never synced; `machine.json`
    /// is its own file with its own atomic save.
    #[serde(skip)]
    machine: MachineStore,
    /// Injectable like `path`, so tests use a temp directory.
    #[serde(skip)]
    pub(crate) attachments_dir: PathBuf,
    /// The automerge document, shared with `TaskStore`. See `sync_doc`.
    #[serde(skip)]
    pub(crate) doc: SyncHandle,
    /// An edit that reached `notes.json` but not yet the document. Without a second
    /// flag the tick would see a clean store and the document would never catch up.
    #[serde(skip)]
    pub(crate) doc_dirty: bool,
    /// Why the corpus failed to load, if it did. Surfaced in the status log on first render.
    #[serde(skip)]
    pub load_error: Option<String>,
    /// Document entries that could not be read back into a `Note`. Kept so the next
    /// reconcile does not prune them and `machine.gc` does not wipe their window state.
    #[serde(skip)]
    pub(crate) unreadable_notes: Vec<String>,
    /// Whether a sync server is configured (set once at startup). Gates whether
    /// `release_attachment_bytes` may delete unreferenced local bytes.
    #[serde(skip)]
    pub(crate) sync_enabled: bool,
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
            sync_enabled: false,
        }
    }
}

/// Monotonic within a process run, so same-millisecond ids stay distinct without a
/// uuid dependency. Local-only ids (temp filenames); anything that syncs uses
/// `next_synced_id`, and note ids use `next_note_id`.
pub(crate) fn next_id() -> String {
    let (millis, n) = raw_id_parts();
    format!("{millis:x}-{n:04x}")
}

/// Same counter as `next_id`, plus a per-install suffix. Task and attachment
/// ids sync across machines keyed by id in the shared document, so they need
/// the same cross-machine uniqueness note ids got — two machines extracting
/// in the same millisecond must not mint the same task id.
pub(crate) fn next_synced_id(machine: &str) -> String {
    let (millis, n) = raw_id_parts();
    format_note_id(millis, n, machine)
}

/// Same counter as `next_id`, plus a per-install suffix. Without it, two machines
/// creating a note in the same millisecond would mint the same id, and a sync merge
/// would silently reparent one machine's tasks onto the other's note.
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

    /// The shared document handle, so `TaskStore` can hold the other half of the same corpus.
    pub fn sync_doc(&self) -> SyncHandle {
        self.doc.clone()
    }

    /// Record whether a sync server is configured. Called once at startup.
    pub fn set_sync_enabled(&mut self, enabled: bool) {
        self.sync_enabled = enabled;
    }

    /// Whether anything is waiting on the JSON mirror or the document.
    pub fn needs_flush(&self) -> bool {
        self.is_dirty() || self.doc_dirty
    }

    /// Whether the document file changed since our last write (how a synced-in copy is noticed).
    pub fn doc_file_moved(&self) -> bool {
        self.doc.lock().file_moved()
    }

    /// Whether the document is unreadable, so nothing derived from it may be written.
    pub fn document_read_only(&self) -> bool {
        self.doc.lock().is_read_only()
    }

    /// Paths taken explicitly so tests avoid the real config dir. GC runs only on a
    /// successful read: a missing/unreadable file is not proof any note is gone.
    fn load_from(
        path: PathBuf,
        machine_path: PathBuf,
        attachments_dir: PathBuf,
        doc_path: PathBuf,
    ) -> Self {
        let machine = MachineStore::load_from(machine_path);
        let (mut doc, load_error) = SyncDoc::open(doc_path);
        // Derived from `path` so tests stay in their temp dir. Only caller that opts in.
        if let Some(dir) = path.parent() {
            doc.set_vocab_path(dir.join("vocabulary.txt"));
        }
        // A document on disk is the corpus; `notes.json` is never read again after that.
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
                sync_enabled: false,
            };
            store.migrate_legacy_attachments();
            // Unreadable entries are read failures, not deletions: spare their window
            // state. Skipped entirely when the document could not be read at all.
            if !store.document_read_only() {
                let valid: std::collections::HashSet<&str> = store
                    .notes
                    .iter()
                    .map(|n| n.id.as_str())
                    .chain(store.unreadable_notes.iter().map(String::as_str))
                    .collect();
                store.machine.gc(&valid);
            }
            return store;
        }

        let mut store = Self::seed_from_json(path, machine, attachments_dir, handle);
        // Seeding happens at most once per install; a second machine gets the
        // document through sync, never by seeding its own copy.
        store.doc_dirty = true;
        store.load_error = load_error.or_else(|| store.load_error.take());
        store
    }

    /// Persist atomically via a sibling temp file + rename, so a crash mid-write
    /// leaves the previous file intact rather than a half-written one.
    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let contents = serde_json::to_string_pretty(self)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, contents)?;
        if let Err(e) = sync_doc::rename_with_retry(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Write only if something changed (driven by a ~500ms tick so keystrokes
    /// coalesce). The two files fail independently: one error retries next tick.
    pub fn flush_if_dirty(&mut self) -> bool {
        if self.document_read_only() {
            // This store came up empty; writing `notes.json` from it would destroy
            // the only other copy. Flags are cleared so the tick does not retry forever.
            // `machine.json` still flushes: window geometry is machine-local and additive.
            self.dirty = false;
            self.doc_dirty = false;
            return self.machine.flush_if_dirty();
        }
        let mut wrote = false;
        if self.dirty {
            // The document write itself belongs to the tick (see `flush`); just mark it owed.
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

    /// Whether either store has an edit pending a write.
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.machine.is_dirty()
    }

    /// This install's id suffix, for synced ids minted outside `notes/`
    /// (attachment ids from the UI).
    pub(crate) fn machine_id(&self) -> &str {
        &self.machine.machine_id
    }

    /// `origin` is explicit: a silent default would corrupt corpus provenance.
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

    /// Machine-local; no production caller yet. Kept because it is part of the
    /// persisted schema and honoured natively on Windows.
    #[allow(dead_code)]
    pub fn pos(&self, id: &str) -> Option<(i32, i32)> {
        self.machine.pos(id)
    }

    pub fn size(&self, id: &str) -> Option<(u32, u32)> {
        self.machine.size(id)
    }

    pub fn is_open(&self, id: &str) -> bool {
        self.machine.is_open(id)
    }

    /// Machine write: does not bump `modified`. No production caller yet; see `pos`.
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

    /// Machine-local; does **not** call `touch()`. Bumping `modified` here let a
    /// window close win a sync merge over a real edit on another machine.
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

    /// Return an archived note to the active list. Leaves `open` alone.
    pub fn restore(&mut self, id: &str) {
        if self.get(id).is_none_or(|n| !n.archived) {
            return;
        }
        if let Some(note) = self.touch(id) {
            note.archived = false;
            self.dirty = true;
        }
    }

    pub fn active(&self) -> Vec<&Note> {
        Self::newest_first(self.notes.iter().filter(|n| !n.archived).collect())
    }

    pub fn archived(&self) -> Vec<&Note> {
        Self::newest_first(self.notes.iter().filter(|n| n.archived).collect())
    }

    /// Active notes matching `query`, newest first. Searches `raw` too (cleanup can
    /// rewrite away spoken words) and matches `body` through `blocks::plain_text`
    /// so attachment tokens are not searchable text.
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
#[path = "migration_tests.rs"]
mod migration_tests;

#[cfg(test)]
#[path = "sync_tests.rs"]
mod sync_tests;

#[cfg(test)]
#[path = "sync_doc_tests.rs"]
mod sync_doc_tests;

#[cfg(test)]
#[path = "sync_recovery_tests.rs"]
mod sync_recovery_tests;
