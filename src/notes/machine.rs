//! Machine-local note state.
//!
//! `pos`, `size` and `open` describe a window on one desktop, not a note's
//! content, and belong here rather than on `Note`. `open` used to be the
//! trap: `NoteStore::set_open` called `touch()`, which rewrote `modified`, so
//! under any last-write-wins merge closing a sticky on one machine made that
//! note look newer than a real edit made on another and won a merge it had
//! no business winning. `size` had the milder version of the same problem:
//! resizing a window generated sync churn with no content change.
//!
//! This store also carries `machine_id`, the short per-install suffix
//! `notes::next_note_id` appends to every note id it mints. Two machines
//! creating their first note in the same millisecond would otherwise produce
//! the same id, and `Task.note_id` is a foreign key into that namespace, so a
//! collision would silently reparent tasks onto the wrong note.
//!
//! Persisted at `<config_dir>/machine.json`, right beside `notes.json`, and
//! **never synced**. That is the entire point of splitting it out.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;

/// One note's window: where it was, how big, and whether it is showing.
///
/// `Default` gives `open: false`, which is correct for an id nothing has
/// touched yet (`NoteStore::create` sets it explicitly, and nothing else
/// should default a note to visible).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    #[serde(default)]
    pub pos: Option<(i32, i32)>,
    #[serde(default)]
    pub size: Option<(u32, u32)>,
    #[serde(default)]
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineStore {
    pub machine_id: String,
    #[serde(default)]
    windows: HashMap<String, WindowState>,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    dirty: bool,
}

/// Used only as a transient placeholder while `serde_json` fills in the
/// non-skipped fields during deserialization; every real caller overwrites
/// `path` immediately after. No disk I/O, so it is safe as a `#[serde(skip)]`
/// default source.
impl Default for MachineStore {
    fn default() -> Self {
        Self { machine_id: String::new(), windows: HashMap::new(), path: PathBuf::new(), dirty: false }
    }
}

impl MachineStore {
    /// Four hex digits, good enough to make two installs' first note tell
    /// apart. Not a uuid, on purpose (see `notes::next_note_id`):
    /// `RandomState` is already randomly seeded per instance for
    /// hash-flooding resistance, and salting its hasher with the wall clock
    /// and this process's id costs nothing and adds real entropy on top.
    fn generate_machine_id() -> String {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_i64(chrono::Local::now().timestamp_nanos_opt().unwrap_or_default());
        hasher.write_u32(std::process::id());
        format!("{:04x}", (hasher.finish() & 0xffff) as u16)
    }

    /// A fresh, in-memory store at `path`, seeded with a new machine id. No
    /// disk I/O. The caller decides when (or whether) to persist it.
    pub fn new(path: PathBuf) -> Self {
        Self { machine_id: Self::generate_machine_id(), windows: HashMap::new(), path, dirty: false }
    }

    /// Load from `path` (always `NoteStore::machine_storage_path()` in
    /// production, an injectable temp path in tests, per `NoteStore::load_from`).
    /// Generates a fresh machine id (and marks the store dirty, so the id
    /// survives the next flush) when the file is missing, unreadable, or
    /// corrupt. A corrupt file is quarantined rather than overwritten,
    /// matching `NoteStore::load`.
    pub fn load_from(path: PathBuf) -> Self {
        if !path.exists() {
            return Self { dirty: true, ..Self::new(path) };
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read machine state at {:?}: {}", path, e);
                return Self { dirty: true, ..Self::new(path) };
            }
        };
        match serde_json::from_str::<MachineStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store.dirty = false;
                store
            }
            Err(e) => {
                let backup = path.with_extension("json.corrupt");
                tracing::error!(
                    "Machine state at {:?} is not valid JSON ({}); preserving as {:?}",
                    path, e, backup
                );
                let _ = std::fs::rename(&path, &backup);
                Self { dirty: true, ..Self::new(path) }
            }
        }
    }

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

    pub fn flush_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        if let Err(e) = self.save() {
            tracing::error!("Failed to save machine state: {}", e);
            return false;
        }
        self.dirty = false;
        true
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// No production caller yet. Reachable through `NoteStore::pos`, which
    /// carries the full explanation. Exercised directly in this module's
    /// tests.
    #[allow(dead_code)]
    pub fn pos(&self, id: &str) -> Option<(i32, i32)> {
        self.windows.get(id).and_then(|w| w.pos)
    }

    pub fn size(&self, id: &str) -> Option<(u32, u32)> {
        self.windows.get(id).and_then(|w| w.size)
    }

    pub fn is_open(&self, id: &str) -> bool {
        self.windows.get(id).is_some_and(|w| w.open)
    }

    /// No production caller yet. See `pos`'s doc comment.
    #[allow(dead_code)]
    pub fn set_pos(&mut self, id: &str, pos: (i32, i32)) {
        let w = self.windows.entry(id.to_string()).or_default();
        if w.pos == Some(pos) {
            return;
        }
        w.pos = Some(pos);
        self.dirty = true;
    }

    pub fn set_size(&mut self, id: &str, size: (u32, u32)) {
        let w = self.windows.entry(id.to_string()).or_default();
        if w.size == Some(size) {
            return;
        }
        w.size = Some(size);
        self.dirty = true;
    }

    pub fn set_open(&mut self, id: &str, open: bool) {
        let w = self.windows.entry(id.to_string()).or_default();
        if w.open == open {
            return;
        }
        w.open = open;
        self.dirty = true;
    }

    /// Drop the window entry for a note that was deleted outright, so
    /// `machine.json` does not keep growing for a note that no longer
    /// exists. The general safety net is `gc`, run on every load; this is
    /// the immediate version for `NoteStore::delete`.
    pub fn remove(&mut self, id: &str) {
        if self.windows.remove(id).is_some() {
            self.dirty = true;
        }
    }

    /// Drop entries for ids not in `valid`. Run on every `NoteStore::load` so
    /// a note deleted (on this machine, or synced as a deletion from another)
    /// does not leave its window state behind forever.
    pub fn gc(&mut self, valid: &HashSet<&str>) {
        let before = self.windows.len();
        self.windows.retain(|id, _| valid.contains(id.as_str()));
        if self.windows.len() != before {
            self.dirty = true;
        }
    }

    /// Lift a legacy note's `pos`/`size`/`open` into this store, for the
    /// `notes.json` migration in `NoteStore::load`.
    ///
    /// Only fills a window with no entry yet. After the first migration,
    /// `Note` no longer serializes these fields, so a legacy notes.json is
    /// naturally read only once in practice, but the guard also means a
    /// second load before the first save (say, a crash in between) cannot
    /// clobber real window state that arrived in the meantime with stale
    /// `None`s and `false`s reconstructed from the same old file.
    pub fn migrate_legacy(&mut self, id: &str, pos: Option<(i32, i32)>, size: Option<(u32, u32)>, open: bool) {
        if self.windows.contains_key(id) {
            return;
        }
        self.windows.insert(id.to_string(), WindowState { pos, size, open });
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("beamer_machine_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.json"));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_fresh_store_gets_a_machine_id_and_no_windows() {
        let store = MachineStore::new(temp_path("fresh"));
        assert!(!store.machine_id.is_empty());
        assert!(!store.is_open("anything"));
        assert_eq!(store.size("anything"), None);
        assert_eq!(store.pos("anything"), None);
    }

    #[test]
    fn set_open_set_size_and_set_pos_round_trip_through_disk() {
        let path = temp_path("roundtrip");
        let mut store = MachineStore::new(path.clone());
        store.set_open("n1", true);
        store.set_size("n1", (320, 260));
        store.set_pos("n1", (100, 200));
        assert!(store.flush_if_dirty());

        let reloaded = MachineStore::load_from(path);
        assert!(reloaded.is_open("n1"));
        assert_eq!(reloaded.size("n1"), Some((320, 260)));
        assert_eq!(reloaded.pos("n1"), Some((100, 200)));
        assert_eq!(reloaded.machine_id, store.machine_id, "the id must survive a reload");
    }

    #[test]
    fn an_unchanged_value_does_not_dirty_the_store() {
        let mut store = MachineStore::new(temp_path("noop"));
        store.set_open("n1", true);
        assert!(store.flush_if_dirty());

        store.set_open("n1", true);
        assert!(!store.is_dirty(), "a redundant set_open must not schedule a write");

        store.set_size("n1", (10, 10));
        store.flush_if_dirty();
        store.set_size("n1", (10, 10));
        assert!(!store.is_dirty());
    }

    #[test]
    fn gc_drops_entries_for_notes_that_no_longer_exist() {
        let mut store = MachineStore::new(temp_path("gc"));
        store.set_open("keep", true);
        store.set_open("gone", true);
        store.flush_if_dirty();

        let valid: HashSet<&str> = ["keep"].into_iter().collect();
        store.gc(&valid);

        assert!(store.is_open("keep"));
        assert!(!store.is_open("gone"), "an entry for a deleted note must be dropped");
        assert!(store.is_dirty(), "the gc itself is a change that must reach disk");
    }

    #[test]
    fn gc_with_nothing_to_drop_does_not_dirty_the_store() {
        let mut store = MachineStore::new(temp_path("gc_noop"));
        store.set_open("keep", true);
        store.flush_if_dirty();

        let valid: HashSet<&str> = ["keep"].into_iter().collect();
        store.gc(&valid);

        assert!(!store.is_dirty(), "nothing was dropped, so nothing needs to be written");
    }

    #[test]
    fn migrate_legacy_fills_an_empty_entry_but_never_clobbers_a_real_one() {
        let mut store = MachineStore::new(temp_path("migrate"));

        store.migrate_legacy("n1", Some((10, 20)), Some((300, 200)), true);
        assert_eq!(store.pos("n1"), Some((10, 20)));
        assert_eq!(store.size("n1"), Some((300, 200)));
        assert!(store.is_open("n1"));

        // The user has since resized the window; a second migration pass
        // (say, from a load before the first save landed) must not overwrite
        // that with the old file's numbers.
        store.set_size("n1", (400, 300));
        store.migrate_legacy("n1", Some((10, 20)), Some((300, 200)), true);
        assert_eq!(store.size("n1"), Some((400, 300)), "an existing entry must not be clobbered");
    }

    #[test]
    fn two_machine_ids_are_never_the_same_by_construction() {
        // Not a statistical claim about the generator, just a sanity check that
        // `new` actually calls it rather than returning a constant.
        let a = MachineStore::new(temp_path("id_a")).machine_id;
        let b = MachineStore::new(temp_path("id_b")).machine_id;
        assert_ne!(a, b, "two fresh installs must not draw the same id in practice");
    }
}
