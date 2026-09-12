//! One-time seed off the old JSON-only storage. Runs only when no `notes.automerge`
//! exists yet: lifts `pos`/`size`/`open` into `machine.json`, then seeds a fresh
//! document. Unknown keys (including the removed `attachments` array) are
//! ignored by `Note`'s own deserialize, so pre-removal files load as-is.

use std::path::PathBuf;

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
    pub(super) fn seed_from_json(path: PathBuf, machine: MachineStore, doc: SyncHandle) -> Self {
        let empty = |path: PathBuf, machine: MachineStore, doc: SyncHandle| {
            Self {
                notes: Vec::new(),
                path,
                dirty: false,
                machine,
                doc,
                doc_dirty: false,
                load_error: None,
                unreadable_notes: Vec::new(),
            }
        };

        if !path.exists() {
            return empty(path, machine, doc);
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                let message = format!("Could not read notes at {}: {e}", path.display());
                tracing::error!("{message}");
                let mut store = empty(path, machine, doc);
                store.load_error = Some(message);
                return store;
            }
        };
        match serde_json::from_str::<NoteStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store.machine = machine;
                store.doc = doc;
                store.migrate_legacy_window_state(&contents);
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
                let mut store = empty(path, machine, doc);
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
}
