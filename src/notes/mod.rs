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

mod model;
pub use model::{Note, NoteColor, NoteState};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteStore {
    pub notes: Vec<Note>,
    #[serde(skip)]
    pub(crate) path: PathBuf,
    #[serde(skip)]
    pub(crate) dirty: bool,
}

impl Default for NoteStore {
    fn default() -> Self {
        Self { notes: Vec::new(), path: Self::storage_path(), dirty: false }
    }
}

/// Monotonic within a process run, so two notes created in the same
/// millisecond still get distinct ids without pulling in a uuid dependency.
fn next_id() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let millis = Local::now().timestamp_millis();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{millis:x}-{n:04x}")
}

impl NoteStore {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("notes.json")
    }

    pub fn load() -> Self {
        let path = Self::storage_path();
        if !path.exists() {
            return Self::default();
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read notes at {:?}: {}", path, e);
                return Self::default();
            }
        };
        match serde_json::from_str::<NoteStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store.dirty = false;
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
                Self::default()
            }
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
    pub fn flush_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        if let Err(e) = self.save() {
            tracing::error!("Failed to save notes: {}", e);
            // Stay dirty so the next tick retries rather than losing the edit.
            return false;
        }
        self.dirty = false;
        true
    }

    /// Whether an edit is pending a write. Read before taking a `write()` lock
    /// on the signal so an idle tick does not notify every subscriber.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn create(&mut self, raw: String, color: NoteColor) -> String {
        let now = Local::now().to_rfc3339();
        let id = next_id();
        self.notes.push(Note {
            id: id.clone(),
            created: now.clone(),
            modified: now,
            body: raw.clone(),
            raw,
            state: NoteState::Raw,
            color,
            pos: None,
            size: None,
            open: true,
            archived: false,
        });
        self.dirty = true;
        id
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
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
    /// A no-op when the value is unchanged. The guard matters because the
    /// callers are event handlers, not user edits: without it a redundant
    /// `set_open` would bump `modified`, dirty the store and trigger a write
    /// for a fact that did not change.
    pub fn set_open(&mut self, id: &str, open: bool) {
        if self.get(id).is_none_or(|n| n.open == open) {
            return;
        }
        if let Some(note) = self.touch(id) {
            note.open = open;
            self.dirty = true;
        }
    }

    pub fn archive(&mut self, id: &str) {
        if let Some(note) = self.touch(id) {
            note.archived = true;
            note.open = false;
            self.dirty = true;
        }
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
                    n.body.to_lowercase().contains(&needle)
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
mod tests {
    use super::*;

    /// PID-scoped temp path so concurrent test runs don't race and nothing
    /// touches the real user config dir. Mirrors `ui::history`'s tests.
    fn temp_store(tag: &str) -> NoteStore {
        let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}.json"));
        let _ = std::fs::remove_file(&path);
        NoteStore { notes: Vec::new(), path, dirty: false }
    }

    #[test]
    fn create_returns_a_unique_id_and_seeds_body_from_raw() {
        let mut store = temp_store("create");
        let a = store.create("call the vet".into(), NoteColor::Purple);
        let b = store.create("send invoice".into(), NoteColor::Teal);

        assert_ne!(a, b, "ids must be unique even within the same millisecond");

        let note = store.get(&a).unwrap();
        assert_eq!(note.raw, "call the vet");
        assert_eq!(note.body, "call the vet", "body starts as a copy of raw");
        assert_eq!(note.state, NoteState::Raw);
        assert!(!note.archived);
    }

    #[test]
    fn set_body_never_touches_raw() {
        let mut store = temp_store("raw_immutable");
        let id = store.create("um so call the vet".into(), NoteColor::Purple);

        store.set_body(&id, "Call the vet.".into());

        let note = store.get(&id).unwrap();
        assert_eq!(note.body, "Call the vet.");
        assert_eq!(
            note.raw, "um so call the vet",
            "raw is the only record of what was actually said and must survive cleanup"
        );
    }

    #[test]
    fn archive_hides_from_active_but_retains_the_note() {
        let mut store = temp_store("archive");
        let keep = store.create("keep".into(), NoteColor::Purple);
        let gone = store.create("archive me".into(), NoteColor::Rose);

        store.archive(&gone);

        let active: Vec<&str> = store.active().iter().map(|n| n.raw.as_str()).collect();
        assert_eq!(active, vec!["keep"]);
        assert!(store.get(&gone).is_some(), "archiving must not delete");
        let _ = keep;
    }

    #[test]
    fn flush_writes_only_when_dirty() {
        let mut store = temp_store("debounce");
        store.create("something".into(), NoteColor::Purple);

        assert!(store.flush_if_dirty(), "a pending change must be written");
        assert!(store.path.exists());
        assert!(
            !store.flush_if_dirty(),
            "a second flush with no intervening edit must not rewrite the file"
        );
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let mut store = temp_store("atomic");
        store.create("hello".into(), NoteColor::Purple);
        store.flush_if_dirty();

        assert!(!store.path.with_extension("json.tmp").exists());
    }

    #[test]
    fn notes_round_trip_through_disk() {
        let mut store = temp_store("roundtrip");
        let id = store.create("first".into(), NoteColor::Amber);
        // Set directly rather than through a setter. `pos` and `size` are part
        // of the persisted schema and must survive a round trip, but nothing
        // writes `pos` from window geometry any more and nothing should — see
        // the field's doc comment. A `set_geometry` that did exist would be a
        // trap for the next person, so it was removed with the scope change
        // that dropped position persistence.
        {
            let note = store.notes.iter_mut().find(|n| n.id == id).unwrap();
            note.pos = Some((100, 200));
            note.size = Some((320, 240));
        }
        store.flush_if_dirty();

        let text = std::fs::read_to_string(&store.path).unwrap();
        let reloaded: NoteStore = serde_json::from_str(&text).unwrap();

        assert_eq!(reloaded.notes.len(), 1);
        assert_eq!(reloaded.notes[0].pos, Some((100, 200)));
        assert_eq!(reloaded.notes[0].size, Some((320, 240)));
        assert_eq!(reloaded.notes[0].color, NoteColor::Amber);
    }

    #[test]
    fn set_open_with_an_unchanged_value_does_not_dirty_the_store() {
        let mut store = temp_store("set_open");
        let id = store.create("hello".into(), NoteColor::Purple);
        store.flush_if_dirty();
        let before = store.get(&id).unwrap().modified.clone();

        store.set_open(&id, true); // already true

        assert!(
            !store.is_dirty(),
            "a redundant set_open must not schedule a write — the callers are \
             window events, not user edits"
        );
        assert_eq!(
            store.get(&id).unwrap().modified,
            before,
            "nothing changed, so the modified timestamp must not move"
        );

        store.set_open(&id, false);
        assert!(store.is_dirty(), "a real change must still be persisted");
        assert!(!store.get(&id).unwrap().open);
    }

    #[test]
    fn set_open_on_a_missing_note_is_a_no_op() {
        let mut store = temp_store("set_open_missing");
        store.set_open("nope", true);
        assert!(!store.is_dirty());
    }

    #[test]
    fn search_matches_what_was_said_not_just_what_is_displayed() {
        let mut store = temp_store("search");
        let id = store.create("um so call the vet about biscuit".into(), NoteColor::Purple);
        // A cleanup pass rewrote the body and dropped the filler word.
        store.set_body(&id, "Call the vet about Biscuit.".into());

        assert_eq!(store.search("biscuit").len(), 1, "matching must be case-insensitive");
        assert_eq!(store.search("VET").len(), 1);
        assert_eq!(
            store.search("um so").len(),
            1,
            "raw is searched too — cleanup can remove the very words you remember saying"
        );
        assert!(store.search("mortgage").is_empty());
    }

    #[test]
    fn an_empty_query_returns_every_active_note() {
        let mut store = temp_store("search_empty");
        store.create("one".into(), NoteColor::Purple);
        let gone = store.create("two".into(), NoteColor::Teal);
        store.archive(&gone);

        assert_eq!(store.search("").len(), 1);
        assert_eq!(store.search("   ").len(), 1, "whitespace is not a query");
        assert!(
            store.search("two").is_empty(),
            "archived notes must stay out of the active board"
        );
    }

    #[test]
    fn restore_returns_an_archived_note_to_the_board() {
        let mut store = temp_store("restore");
        let id = store.create("bring me back".into(), NoteColor::Rose);
        store.archive(&id);
        assert_eq!(store.archived().len(), 1);
        assert!(store.active().is_empty());

        store.restore(&id);

        assert!(store.active().iter().any(|n| n.id == id));
        assert!(store.archived().is_empty());
        assert!(
            !store.get(&id).unwrap().open,
            "restoring puts a note back on the board; it does not pop a window open"
        );
    }

    #[test]
    fn restoring_a_note_that_is_not_archived_does_nothing() {
        let mut store = temp_store("restore_noop");
        let id = store.create("already here".into(), NoteColor::Purple);
        store.flush_if_dirty();

        store.restore(&id);

        assert!(!store.is_dirty(), "a no-op restore must not schedule a write");
    }

}
