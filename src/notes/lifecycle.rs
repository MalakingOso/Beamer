//! What the model passes are allowed to do to a note.
//!
//! A second `impl NoteStore` rather than more methods in `mod.rs`, which is
//! already at 459 of the project's 500-line limit. Every method here is a
//! *machine* write: none of them bump `modified` (that is user-facing ordering)
//! and none of them touch `raw` (the only record of what was actually said).

use super::model::StageState;
use super::{Note, NoteStore};

/// What became of a stage result by the time it got back to the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOutcome {
    Applied,
    /// The note changed under the model — the result was thrown away.
    Superseded,
    /// The note was deleted while the pass was in flight.
    NoteGone,
}

impl NoteStore {
    /// Install a cleanup result, but only if the note still says what it said
    /// when the request went out.
    ///
    /// A compare-and-swap on `expected` — the body captured at send time. If
    /// the user typed into the note while the model was thinking, their edit
    /// wins and this returns `Superseded` having changed *nothing*, not even
    /// `clean_state`: leaving it `Pending` keeps the footer's retry affordance
    /// available, which is right for a pass that was superseded rather than one
    /// that failed.
    ///
    /// The guard is body equality, deliberately **not** the `modified`
    /// timestamp: `set_color` and `set_open` bump `modified` for things that
    /// are not edits at all, so a timestamp guard would reject perfectly good
    /// results.
    ///
    /// An empty or whitespace-only `cleaned` is a **success**. A note that was
    /// pure filler correctly cleans up to nothing, and the model saying so must
    /// leave the body exactly as it was rather than blanking the user's note.
    /// The intuitive `if cleaned.is_empty() { failed }` gets this backwards and
    /// destroys content.
    pub fn apply_cleanup(&mut self, id: &str, expected: &str, cleaned: &str) -> StageOutcome {
        let Some(note) = Self::find_mut(&mut self.notes, id) else {
            return StageOutcome::NoteGone;
        };
        if note.body != expected {
            return StageOutcome::Superseded;
        }
        if !cleaned.trim().is_empty() {
            note.body = cleaned.to_string();
        }
        note.clean_state = StageState::Done;
        self.dirty = true;
        StageOutcome::Applied
    }

    pub fn mark_clean_failed(&mut self, id: &str) {
        self.set_clean_state(id, StageState::Failed);
    }

    /// Cleanup was deliberately not run — disabled in config, or a typed note.
    pub fn mark_clean_skipped(&mut self, id: &str) {
        self.set_clean_state(id, StageState::Skipped);
    }

    // The two extraction-result methods below have no caller until the
    // extraction stage is wired; `mark_extract_skipped` already does, via the
    // llm-disabled branch in `notes::pipeline`.
    #[allow(dead_code)]
    pub fn mark_analyzed(&mut self, id: &str) {
        self.set_extract_state(id, StageState::Done);
    }

    #[allow(dead_code)]
    pub fn mark_extract_failed(&mut self, id: &str) {
        self.set_extract_state(id, StageState::Failed);
    }

    /// Extraction was deliberately not run.
    pub fn mark_extract_skipped(&mut self, id: &str) {
        self.set_extract_state(id, StageState::Skipped);
    }

    /// Each stage owns its own field and nothing else. A failed cleanup must
    /// never block or overwrite a successful extraction — that pairing is the
    /// case the old single `NoteState` could not represent.
    fn set_clean_state(&mut self, id: &str, state: StageState) {
        if let Some(note) = Self::find_mut(&mut self.notes, id) {
            note.clean_state = state;
            self.dirty = true;
        }
    }

    fn set_extract_state(&mut self, id: &str, state: StageState) {
        if let Some(note) = Self::find_mut(&mut self.notes, id) {
            note.extract_state = state;
            self.dirty = true;
        }
    }

    /// Deliberately not `mod.rs`'s `touch()`: that bumps `modified`, and the
    /// caller here is a background model pass, not the user. A missing id is a
    /// no-op everywhere in this module and must not dirty the store, matching
    /// `set_open`'s guard.
    fn find_mut<'a>(notes: &'a mut [Note], id: &str) -> Option<&'a mut Note> {
        notes.iter_mut().find(|n| n.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::{NoteColor, NoteOrigin};

    /// PID-scoped temp path so concurrent test runs don't race and nothing
    /// touches the real user config dir. Mirrors `ui::history`'s tests.
    fn temp_store(tag: &str) -> NoteStore {
        let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("lifecycle_{tag}.json"));
        let _ = std::fs::remove_file(&path);
        NoteStore { notes: Vec::new(), path, dirty: false }
    }

    #[test]
    fn empty_cleanup_response_is_success_not_failure() {
        let mut store = temp_store("empty_clean");
        let id = store.create("um, uh, so, yeah".into(), NoteColor::Purple, NoteOrigin::Dictated);

        let outcome = store.apply_cleanup(&id, "um, uh, so, yeah", "   \n  ");

        let note = store.get(&id).unwrap();
        assert_eq!(outcome, StageOutcome::Applied);
        assert_eq!(
            note.clean_state,
            StageState::Done,
            "a note of pure filler correctly cleans to nothing — that is the model working, \
             not failing, and offering a retry would just repeat it"
        );
        assert_ne!(note.clean_state, StageState::Failed);
        assert_eq!(
            note.body, "um, uh, so, yeah",
            "an empty response must never blank the user's note"
        );
        assert_eq!(note.body, note.raw);
    }

    #[test]
    fn superseded_cleanup_never_clobbers_an_edit() {
        let mut store = temp_store("cas");
        let id = store.create("call the vet".into(), NoteColor::Purple, NoteOrigin::Dictated);
        // Body captured at send time; the user then types while the model thinks.
        let sent_with = "call the vet".to_string();
        store.set_body(&id, "call the vet about Biscuit".into());
        store.flush_if_dirty();

        let outcome = store.apply_cleanup(&id, &sent_with, "Call the vet.");

        let note = store.get(&id).unwrap();
        assert_eq!(outcome, StageOutcome::Superseded);
        assert_eq!(
            note.body, "call the vet about Biscuit",
            "the user's edit outranks a result computed from text they have moved past"
        );
        assert_eq!(
            note.clean_state,
            StageState::Pending,
            "superseded is not failed — leaving it Pending keeps the retry affordance live"
        );
        assert!(!store.is_dirty(), "a discarded result must not schedule a write");
    }

    #[test]
    fn a_failed_cleanup_still_permits_a_successful_extraction() {
        let mut store = temp_store("independent");
        let id = store.create("ship the invoice".into(), NoteColor::Teal, NoteOrigin::Dictated);

        store.mark_clean_failed(&id);
        store.mark_analyzed(&id);

        let note = store.get(&id).unwrap();
        assert_eq!(note.clean_state, StageState::Failed);
        assert_eq!(
            note.extract_state,
            StageState::Done,
            "the stages are independent — a cleanup outage must not cost the user their tasks"
        );
    }

    #[test]
    fn old_notes_without_stage_fields_load_as_pending() {
        // A note exactly as v1 wrote it: one linear `state`, no stage fields.
        let json = r#"{
            "id": "18f2a1b3-0001",
            "created": "2026-08-01T09:15:00+01:00",
            "modified": "2026-08-01T09:15:00+01:00",
            "raw": "call the vet about biscuit",
            "body": "call the vet about biscuit",
            "state": "raw",
            "color": "amber",
            "pos": null,
            "size": null,
            "open": true,
            "archived": false
        }"#;

        let note: Note = serde_json::from_str(json)
            .expect("an existing notes.json must keep loading — migration is free and stays free");

        assert_eq!(note.clean_state, StageState::Pending);
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
    fn applying_a_cleanup_rewrites_the_body_and_leaves_raw_alone() {
        let mut store = temp_store("apply");
        let id = store.create("um so call the vet".into(), NoteColor::Purple, NoteOrigin::Dictated);

        let outcome = store.apply_cleanup(&id, "um so call the vet", "Call the vet.");

        let note = store.get(&id).unwrap();
        assert_eq!(outcome, StageOutcome::Applied);
        assert_eq!(note.body, "Call the vet.");
        assert_eq!(
            note.raw, "um so call the vet",
            "raw is the only record of what was actually said and must survive cleanup"
        );
        assert!(store.is_dirty(), "the rewrite must reach disk on the next debounce tick");
    }

    #[test]
    fn a_machine_write_does_not_move_the_modified_timestamp() {
        let mut store = temp_store("modified");
        let id = store.create("draft the reply".into(), NoteColor::Slate, NoteOrigin::Dictated);
        let before = store.get(&id).unwrap().modified.clone();

        store.apply_cleanup(&id, "draft the reply", "Draft the reply.");
        store.mark_analyzed(&id);
        store.mark_clean_skipped(&id);

        assert_eq!(
            store.get(&id).unwrap().modified,
            before,
            "modified is user-facing ordering on the board; a background pass is not an edit"
        );
    }

    #[test]
    fn a_stage_result_for_a_deleted_note_is_a_no_op() {
        let mut store = temp_store("gone");

        assert_eq!(store.apply_cleanup("nope", "anything", "cleaned"), StageOutcome::NoteGone);
        store.mark_clean_failed("nope");
        store.mark_clean_skipped("nope");
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
        store.mark_clean_skipped(&id);
        store.mark_extract_failed(&id);
        store.flush_if_dirty();

        let text = std::fs::read_to_string(&store.path).unwrap();
        let reloaded: NoteStore = serde_json::from_str(&text).unwrap();

        assert_eq!(reloaded.notes[0].clean_state, StageState::Skipped);
        assert_eq!(reloaded.notes[0].extract_state, StageState::Failed);
        assert_eq!(reloaded.notes[0].origin, NoteOrigin::Dictated);
    }
}
