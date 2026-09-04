//! One-time seed off the old JSON-only storage. Runs only when no `notes.automerge`
//! exists yet: upgrades the legacy attachment shape, lifts `pos`/`size`/`open` into
//! `machine.json`, and seeds a fresh document.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::sync_doc::SyncHandle;
use super::{MachineStore, NoteStore};

/// Window-state fields a legacy `notes.json` carries. Parsed separately from `Note`
/// (which ignores these keys) so the migration can lift them into `machine.json`.
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
    /// Seed a fresh document from a pre-document `notes.json`.
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
                    unreadable_notes: Vec::new(),
                    sync_enabled: false,
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
        // Independent of whether any bytes were adoptable: a missing file still gets
        // the shape upgrade, and without the flag the old shape would linger on disk.
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
                // Never start empty on corrupt input, or the next write destroys the notes.
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

    /// Lift `pos`/`size`/`open` into `machine.json`. Dirties the store when anything
    /// moves, so the next flush rewrites `notes.json` without the stale keys — a
    /// synced mirror must never carry one machine's geometry to another.
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

    /// Rewrite legacy path-shaped attachments into the `{filename, location}` shape as
    /// a JSON transform, so `Attachment`'s `Deserialize` never learns the old shape.
    /// Without it, any pre-migration file would fail the strict parse and be quarantined.
    /// Returns the text to parse plus whether anything changed (re-serialization alone
    /// changes formatting, so the flag cannot be read off a string comparison).
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

    /// Rewrite one attachment object in place if legacy. Returns whether it changed.
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
