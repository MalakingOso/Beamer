//! Machine-local note state (`pos`/`size`/`open`): window geometry, not content.
//! Persisted at `<config_dir>/machine.json` and **never synced** — syncing it let a
//! window close win a merge over a real edit. Also carries `machine_id`, the
//! per-install suffix keeping note ids unique across machines.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;

/// One note's window: where it was, how big, whether it is showing.
/// `Default` is `open: false` (`create` sets it explicitly).
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
    /// Set when `load_from` could not even stat the file (not merely missing).
    /// The in-memory store must then never reach disk over whatever is really there.
    #[serde(skip)]
    read_only: bool,
}

impl Default for MachineStore {
    fn default() -> Self {
        Self {
            machine_id: String::new(),
            windows: HashMap::new(),
            path: PathBuf::new(),
            dirty: false,
            read_only: false,
        }
    }
}

impl MachineStore {
    /// Four hex digits distinguishing installs. Not a uuid on purpose: `RandomState`
    /// is already per-instance seeded, salted here with wall clock + pid.
    fn generate_machine_id() -> String {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_i64(chrono::Local::now().timestamp_nanos_opt().unwrap_or_default());
        hasher.write_u32(std::process::id());
        format!("{:04x}", (hasher.finish() & 0xffff) as u16)
    }

    /// A fresh in-memory store at `path`. No disk I/O; the caller decides when to persist.
    pub fn new(path: PathBuf) -> Self {
        Self {
            machine_id: Self::generate_machine_id(),
            windows: HashMap::new(),
            path,
            dirty: false,
            read_only: false,
        }
    }

    /// Missing/unreadable/corrupt files mint a fresh id (dirty, so it persists);
    /// corrupt ones are quarantined. A path that cannot even be stat-ed latches
    /// `read_only` instead — it is not the same as missing.
    pub fn load_from(path: PathBuf) -> Self {
        match path.try_exists() {
            Ok(false) => return Self { dirty: true, ..Self::new(path) },
            Err(e) => {
                tracing::error!(
                    "Could not tell whether machine state at {:?} exists ({}); leaving it \
                     alone rather than risk overwriting it with a fresh, empty store",
                    path, e
                );
                return Self { dirty: false, read_only: true, ..Self::new(path) };
            }
            Ok(true) => {}
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
        if let Err(e) = super::sync_doc::rename_with_retry(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    pub fn flush_if_dirty(&mut self) -> bool {
        if self.read_only {
            // The real file's absence was never confirmed: nothing in memory may reach disk.
            self.dirty = false;
            return false;
        }
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

    /// No production caller yet; see `NoteStore::pos`.
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

    /// No production caller yet; see `pos`.
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

    /// Drop a deleted note's window entry now (`gc` on load is the safety net).
    pub fn remove(&mut self, id: &str) {
        if self.windows.remove(id).is_some() {
            self.dirty = true;
        }
    }

    /// Drop entries for ids not in `valid`. Run on every `NoteStore::load`.
    pub fn gc(&mut self, valid: &HashSet<&str>) {
        let before = self.windows.len();
        self.windows.retain(|id, _| valid.contains(id.as_str()));
        if self.windows.len() != before {
            self.dirty = true;
        }
    }

    /// Lift a legacy note's `pos`/`size`/`open` into this store. Only fills an empty
    /// entry, so a second load before the first save cannot clobber newer state.
    /// Returns whether anything was inserted (caller then rewrites `notes.json`).
    pub fn migrate_legacy(&mut self, id: &str, pos: Option<(i32, i32)>, size: Option<(u32, u32)>, open: bool) -> bool {
        if self.windows.contains_key(id) {
            return false;
        }
        self.windows.insert(id.to_string(), WindowState { pos, size, open });
        self.dirty = true;
        true
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

        assert!(store.migrate_legacy("n1", Some((10, 20)), Some((300, 200)), true));
        assert_eq!(store.pos("n1"), Some((10, 20)));
        assert_eq!(store.size("n1"), Some((300, 200)));
        assert!(store.is_open("n1"));

        store.set_size("n1", (400, 300));
        assert!(
            !store.migrate_legacy("n1", Some((10, 20)), Some((300, 200)), true),
            "an id already present must report nothing was inserted"
        );
        assert_eq!(store.size("n1"), Some((400, 300)), "an existing entry must not be clobbered");
    }

    #[test]
    fn two_machine_ids_are_never_the_same_by_construction() {
        let a = MachineStore::new(temp_path("id_a")).machine_id;
        let b = MachineStore::new(temp_path("id_b")).machine_id;
        assert_ne!(a, b, "two fresh installs must not draw the same id in practice");
    }
}
