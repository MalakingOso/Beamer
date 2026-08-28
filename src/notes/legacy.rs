//! The one-time path off the old JSON-only storage.
//!
//! Everything here runs when there is no `notes.automerge` yet: the legacy
//! attachment shape is upgraded, `pos`/`size`/`open` are lifted into
//! `machine.json`, and the resulting notes seed a fresh document. Once the
//! document exists, `NoteStore::load_from` reads that and none of this runs
//! again.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::sync_doc::SyncHandle;
use super::{MachineStore, NoteStore};

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
    /// The pre-document load path, now used only to seed a fresh document
    /// from a `notes.json` written before this task.
    pub(super) fn seed_from_json(
        path: PathBuf,
        machine: MachineStore,
        attachments_dir: PathBuf,
        doc: SyncHandle,
    ) -> Self {
        let empty =
            |path: PathBuf, machine: MachineStore, attachments_dir: PathBuf, doc: SyncHandle| {
                Self {
                    notes: Vec::new(),
                    path,
                    dirty: false,
                    machine,
                    attachments_dir,
                    doc,
                    doc_dirty: false,
                    load_error: None,
                }
            };

        if !path.exists() {
            return empty(path, machine, attachments_dir, doc);
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                let message = format!("Could not read notes at {}: {e}", path.display());
                tracing::error!("{message}");
                let mut store = empty(path, machine, attachments_dir, doc);
                store.load_error = Some(message);
                return store;
            }
        };
        // `shape_upgraded` is independent of whether `migrate_legacy_attachments`
        // below finds anything it can actually adopt: a note whose attachment
        // file is missing gets the shape upgrade but no hash, so without this
        // the store would never dirty and `notes.json` would carry the old
        // `path`-only shape forever, which `task_eval`'s own strict,
        // non-upgrading parse of the same file cannot read.
        let (upgraded, shape_upgraded) = Self::upgrade_legacy_attachment_shape(&contents);
        match serde_json::from_str::<NoteStore>(&upgraded) {
            Ok(mut store) => {
                store.path = path;
                store.dirty = shape_upgraded;
                store.machine = machine;
                store.attachments_dir = attachments_dir;
                store.doc = doc;
                store.migrate_legacy_window_state(&contents);
                store.migrate_legacy_attachments();
                let valid: std::collections::HashSet<&str> =
                    store.notes.iter().map(|n| n.id.as_str()).collect();
                store.machine.gc(&valid);
                store
            }
            Err(e) => {
                // Same reasoning as history.rs: don't start empty, or the next
                // write destroys the user's notes for good.
                let backup = path.with_extension("json.corrupt");
                let message = format!(
                    "Notes at {} are not valid JSON ({e}); preserved as {}",
                    path.display(),
                    backup.display()
                );
                tracing::error!("{message}");
                let _ = std::fs::rename(&path, &backup);
                let mut store = empty(path, machine, attachments_dir, doc);
                store.load_error = Some(message);
                store
            }
        }
    }

    /// Lift `pos`/`size`/`open` off a `notes.json` written before this task,
    /// into `machine.json`. A no-op once every note in the file has been
    /// migrated once (`MachineStore::migrate_legacy` will not overwrite an
    /// existing entry), and a no-op forever after the first save, since
    /// `Note` stops serializing these fields at all.
    ///
    /// Sets `self.dirty` when anything was actually lifted, so the next flush
    /// rewrites `notes.json` without the stale keys. Leaving them on disk
    /// looked harmless locally (an unrelated edit would eventually flush them
    /// away), but once `notes.json` syncs (Task 10), a legacy file that never
    /// gets a content edit before it reaches a second machine would carry its
    /// `pos`/`size`/`open` there and let that machine's own migration import
    /// the first machine's geometry and open set. Machine-local state leaking
    /// through the synced file is exactly what this task exists to prevent.
    fn migrate_legacy_window_state(&mut self, contents: &str) {
        let Ok(legacy) = serde_json::from_str::<LegacyNotesFile>(contents) else {
            return;
        };
        for note in legacy.notes {
            if self.machine.migrate_legacy(&note.id, note.pos, note.size, note.open) {
                self.dirty = true;
            }
        }
    }

    /// Rewrite any legacy, path-shaped attachment object in `contents` into
    /// the current `{filename, location}` shape, as a JSON transform rather
    /// than a Rust-level one.
    ///
    /// `Attachment`'s own `Deserialize` only ever accepts the current shape:
    /// it has no reason to know about the old one, and giving it one would
    /// mean carrying that knowledge in `model.rs` forever. Without this
    /// upgrade step, a `notes.json` written before this task would fail
    /// `Note`'s parse, which fails `NoteStore`'s parse, and the whole file
    /// would be quarantined as corrupt: the exact loss `load_from`'s error
    /// path exists to prevent, on every install that predates this task.
    ///
    /// A no-op on a file that is already current: every attachment it walks
    /// already carries `location`, so nothing is rewritten. Falls back to
    /// `contents` unchanged if it is not even valid JSON, leaving the
    /// subsequent strict parse to fail exactly as it always has.
    ///
    /// Returns whether anything was actually rewritten, alongside the text to
    /// parse. Re-serializing through `serde_json::Value` changes formatting
    /// even when the data is identical (compact rather than the pretty-print
    /// `save()` writes), so that can never be read off a plain string
    /// comparison against `contents`; this flag is the only honest signal.
    fn upgrade_legacy_attachment_shape(contents: &str) -> (String, bool) {
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(contents) else {
            return (contents.to_string(), false);
        };
        let mut changed = false;
        if let Some(notes) = value.get_mut("notes").and_then(|n| n.as_array_mut()) {
            for note in notes {
                let Some(attachments) = note.get_mut("attachments").and_then(|a| a.as_array_mut())
                else {
                    continue;
                };
                for attachment in attachments {
                    changed |= Self::upgrade_one_attachment(attachment);
                }
            }
        }
        if !changed {
            return (contents.to_string(), false);
        }
        match serde_json::to_string(&value) {
            Ok(upgraded) => (upgraded, true),
            Err(_) => (contents.to_string(), false),
        }
    }

    /// Rewrites one attachment object in place if it is in the legacy shape.
    /// Returns whether it changed anything.
    fn upgrade_one_attachment(attachment: &mut serde_json::Value) -> bool {
        let Some(obj) = attachment.as_object_mut() else { return false };
        let kind = obj.get("kind").and_then(|k| k.as_str()).map(str::to_string);
        if !matches!(kind.as_deref(), Some("image") | Some("file")) {
            return false;
        }
        if obj.contains_key("location") {
            return false;
        }
        let Some(path) = obj.remove("path") else { return false };
        let filename = path
            .as_str()
            .map(Path::new)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        obj.entry("filename").or_insert_with(|| serde_json::Value::String(filename));
        obj.insert("location".to_string(), serde_json::json!({ "kind": "external", "path": path }));
        true
    }
}
