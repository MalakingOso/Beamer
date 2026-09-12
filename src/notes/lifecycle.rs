//! Machine writes from the extraction pass: never bump `modified`, never touch `raw`.

use super::model::StageState;
use super::{Note, NoteStore};

impl NoteStore {
    pub fn mark_analyzed(&mut self, id: &str) {
        self.set_extract_state(id, StageState::Done);
    }

    pub fn mark_extract_failed(&mut self, id: &str) {
        self.set_extract_state(id, StageState::Failed);
    }

    /// Extraction deliberately not run (disabled in config). Never overwrites
    /// `Done`: asking for a pass while the feature is off must not erase the
    /// record that it once ran. Enforced here rather than at the call sites,
    /// so no future caller can get it wrong.
    pub fn mark_extract_skipped(&mut self, id: &str) {
        if let Some(note) = Self::find_mut(&mut self.notes, id) {
            if note.extract_state != StageState::Done {
                note.extract_state = StageState::Skipped;
                self.dirty = true;
            }
        }
    }

    fn set_extract_state(&mut self, id: &str, state: StageState) {
        if let Some(note) = Self::find_mut(&mut self.notes, id) {
            note.extract_state = state;
            self.dirty = true;
        }
    }

    /// Not `touch()`: background passes must not bump `modified`. Missing id is
    /// a no-op that does not dirty the store. `pub(super)` so `edit.rs` shares it.
    pub(super) fn find_mut<'a>(notes: &'a mut [Note], id: &str) -> Option<&'a mut Note> {
        notes.iter_mut().find(|n| n.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::{NoteColor, NoteOrigin};

    fn temp_store(tag: &str) -> NoteStore {
        let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("lifecycle_{tag}.json"));
        let machine_path = dir.join(format!("lifecycle_{tag}.machine.json"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&machine_path);
        NoteStore {
            notes: Vec::new(),
            path,
            dirty: false,
            machine: crate::notes::MachineStore::new(machine_path),
            doc: crate::notes::sync_doc::SyncHandle::default(),
            doc_dirty: false,
            load_error: None,
            unreadable_notes: Vec::new(),
        }
    }

    #[test]
    fn skipped_never_overwrites_done() {
        let mut store = temp_store("skipped_done");
        let id = store.create("file the taxes".into(), NoteColor::Rose, NoteOrigin::Dictated);
        store.mark_analyzed(&id);

        store.mark_extract_skipped(&id);

        let note = store.get(&id).unwrap();
        assert_eq!(note.extract_state, StageState::Done);
    }

    #[test]
    fn skipped_downgrades_a_failed_pass_so_a_disabled_feature_goes_quiet() {
        let mut store = temp_store("skipped_failed");
        let id = store.create("file the taxes".into(), NoteColor::Rose, NoteOrigin::Dictated);
        store.mark_extract_failed(&id);

        store.mark_extract_skipped(&id);

        let note = store.get(&id).unwrap();
        assert_eq!(note.extract_state, StageState::Skipped);
    }

    #[test]
    fn old_notes_without_stage_fields_load_as_pending() {
        let json = r#"{
            "id": "18f2a1b3-0001",
            "created": "2026-08-01T09:15:00+01:00",
            "modified": "2026-08-01T09:15:00+01:00",
            "raw": "call the vet about biscuit",
            "body": "call the vet about biscuit",
            "state": "raw",
            "clean_state": "failed",
            "color": "amber",
            "pos": null,
            "size": null,
            "open": true,
            "archived": false
        }"#;

        let note: Note = serde_json::from_str(json)
            .expect("an existing notes.json must keep loading — migration is free and stays free");

        // `clean_state` is a leftover key from the removed cleanup pass: read
        // and ignored, so a pre-strip file loads exactly like this one.
        assert_eq!(note.extract_state, StageState::Pending);
        assert_eq!(
            note.origin,
            NoteOrigin::Dictated,
            "every note written before this change came from dictation; relabelling them Typed \
             would corrupt the provenance of the whole existing corpus"
        );
        assert_eq!(note.raw, "call the vet about biscuit");
        assert_eq!(note.color, NoteColor::Amber);
    }

    #[test]
    fn a_machine_write_does_not_move_the_modified_timestamp() {
        let mut store = temp_store("modified");
        let id = store.create("draft the reply".into(), NoteColor::Slate, NoteOrigin::Dictated);
        let before = store.get(&id).unwrap().modified.clone();

        store.mark_analyzed(&id);
        store.mark_extract_skipped(&id);

        assert_eq!(
            store.get(&id).unwrap().modified,
            before,
            "modified is user-facing ordering on the board; a background pass is not an edit"
        );
    }

    #[test]
    fn a_stage_result_for_a_deleted_note_is_a_no_op() {
        let mut store = temp_store("gone");

        store.mark_analyzed("nope");
        store.mark_extract_failed("nope");
        store.mark_extract_skipped("nope");

        assert!(
            !store.is_dirty(),
            "a result arriving after the note was deleted must not schedule a write"
        );
        assert!(store.notes.is_empty(), "nothing may be resurrected from a stale result");
    }

    #[test]
    fn stage_fields_survive_a_round_trip_through_disk() {
        let mut store = temp_store("roundtrip");
        let id = store.create("file the taxes".into(), NoteColor::Rose, NoteOrigin::Dictated);
        store.mark_extract_failed(&id);
        store.flush_if_dirty();

        let text = std::fs::read_to_string(&store.path).unwrap();
        let reloaded: NoteStore = serde_json::from_str(&text).unwrap();

        assert_eq!(reloaded.notes[0].extract_state, StageState::Failed);
        assert_eq!(reloaded.notes[0].origin, NoteOrigin::Dictated);
    }
}
