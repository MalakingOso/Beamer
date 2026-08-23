//! Editing a note's attachments, its size, and its existence.
//!
//! A third `impl NoteStore`, following `lifecycle.rs`. `mod.rs` is at 470 of
//! the project's 500-line limit and has no room; more importantly these are a
//! coherent group — everything that changes what a note *contains* beyond its
//! text, plus the one operation that removes a note for good.
//!
//! Two conventions carried over from `lifecycle.rs`, both load-bearing:
//!
//! - **`set_size` is a machine write.** It does not bump `modified`. `Resized`
//!   fires per frame during a grip drag and once again when the window maps;
//!   bumping the timestamp would churn the board's newest-first ordering on
//!   every mouse move.
//! - **A missing id is a no-op that does not dirty the store**, matching
//!   `set_open`'s guard, so an event arriving after a note was deleted costs
//!   nothing.
//!
//! ⚠️ **Nothing here ever touches a file on disk.** `Attachment` paths point at
//! the user's own photos and documents; Beamer records where they are and
//! nothing more. Deleting a note, or an attachment, removes a *record*.

use super::blocks;
use super::model::Attachment;
use super::{Note, NoteStore};

impl NoteStore {
    /// Remember how big a note's window is, in **logical** pixels.
    ///
    /// ⚠️ The caller must divide out `scale_factor()` first: `WindowEvent::Resized`
    /// carries physical pixels while the builder consumed a `LogicalSize`. At
    /// scale 1.0 the two agree, so getting this wrong is invisible on a
    /// 1x display and wrong on every HiDPI one.
    ///
    /// A no-op when the size is unchanged, for the same reason `set_open`
    /// guards: the caller is a window event, not a user edit, and any
    /// `notes.write()` at all notifies every subscriber — re-rendering the note
    /// and the board once per frame of a resize drag.
    pub fn set_size(&mut self, id: &str, size: (u32, u32)) {
        if self.get(id).is_none_or(|n| n.size == Some(size)) {
            return;
        }
        if let Some(note) = Self::find_mut(&mut self.notes, id) {
            note.size = Some(size);
            self.dirty = true;
        }
    }

    /// Attach something and append its token to the body.
    ///
    /// The record and the token are written together, in one store write. They
    /// are two halves of one fact, and a note carrying one without the other is
    /// the desynchronised state `blocks` renders as literal text.
    ///
    /// This *is* a user edit, so it does bump `modified` — dropping a photo on
    /// a note is as much a change as typing into it.
    pub fn add_attachment(&mut self, id: &str, attachment: Attachment) {
        let token_id = attachment.id().to_string();
        let Some(note) = self.touch(id) else { return };
        note.body = blocks::insert_token(&note.body, &token_id, true);
        note.attachments.push(attachment);
        self.dirty = true;
    }

    /// Detach something: drop the record and the token together.
    pub fn remove_attachment(&mut self, id: &str, attachment_id: &str) {
        let Some(note) = self.touch(id) else { return };
        note.body = blocks::remove_token(&note.body, attachment_id);
        note.attachments.retain(|a| a.id() != attachment_id);
        self.dirty = true;
    }

    /// Repoint an attachment at a file that moved — the "Locate…" gesture.
    ///
    /// Returns whether anything changed. The id and the token stay as they
    /// were, so the attachment keeps its place in reading order.
    pub fn relocate_attachment(&mut self, id: &str, attachment_id: &str, path: std::path::PathBuf) -> bool {
        let Some(note) = self.touch(id) else { return false };
        let Some(att) = note.attachments.iter_mut().find(|a| a.id() == attachment_id) else {
            return false;
        };
        match att {
            Attachment::Image { path: p, .. } | Attachment::File { path: p, .. } => {
                if *p == path {
                    return false;
                }
                *p = path;
            }
            // A link has no file to relocate. Silently doing nothing is right:
            // the UI never offers "Locate…" on a chip.
            Attachment::Link { .. } => return false,
        }
        self.dirty = true;
        true
    }

    /// Drop `Attachment` records whose token the user deleted from the text.
    ///
    /// Under reference-by-path, Beamer owns no media files, so the only thing
    /// that can be orphaned is a *record*. Deleting the token from a textarea
    /// is the gesture that orphans one, and this is what collects it.
    ///
    /// Returns whether anything was dropped, so a caller on the render path can
    /// avoid a write when there is nothing to do.
    pub fn prune_attachments(&mut self, id: &str) -> bool {
        let Some(note) = Self::find_mut(&mut self.notes, id) else {
            return false;
        };
        let referenced: Vec<String> =
            blocks::referenced_ids(&note.body).into_iter().map(str::to_string).collect();
        let before = note.attachments.len();
        note.attachments.retain(|a| referenced.iter().any(|r| r == a.id()));
        if note.attachments.len() == before {
            return false;
        }
        self.dirty = true;
        true
    }

    /// Remove a note outright. Returns whether one was removed.
    ///
    /// **Real deletion, which this store has never had before.** Archive stays
    /// the default gesture and is non-destructive; this is the deliberate act,
    /// offered only on an already-archived note and only behind a confirm.
    ///
    /// The note's rows in `TaskStore` go with it — see
    /// `TaskStore::delete_for_note`, which the caller invokes alongside this.
    /// That is a deliberate exception to "dismissed rows are retained as
    /// labelled negatives": an explicit delete means gone.
    ///
    /// Files on disk are **never** touched. A note holding a photo is a
    /// reference to that photo, not a copy of it.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        if self.notes.len() == before {
            return false;
        }
        self.dirty = true;
        true
    }

    /// The attachment a token refers to, if the note still holds its record.
    ///
    /// `None` is the desynchronised case, and it is not an error: `blocks`
    /// renders an unmatched token as literal text so the note fails visibly.
    pub fn attachment<'a>(note: &'a Note, attachment_id: &str) -> Option<&'a Attachment> {
        note.attachments.iter().find(|a| a.id() == attachment_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::{NoteColor, NoteOrigin};
    use std::path::PathBuf;

    fn temp_store(tag: &str) -> NoteStore {
        let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("edit_{tag}.json"));
        let _ = std::fs::remove_file(&path);
        NoteStore { notes: Vec::new(), path, dirty: false }
    }

    fn image(id: &str, path: &str) -> Attachment {
        Attachment::Image { id: id.into(), path: PathBuf::from(path), alt: None }
    }

    #[test]
    fn set_size_records_the_size_without_moving_the_modified_timestamp() {
        let mut store = temp_store("size");
        let id = store.create("note".into(), NoteColor::Purple, NoteOrigin::Dictated);
        let before = store.get(&id).unwrap().modified.clone();

        store.set_size(&id, (400, 320));

        assert_eq!(store.get(&id).unwrap().size, Some((400, 320)));
        assert_eq!(
            store.get(&id).unwrap().modified,
            before,
            "Resized fires per frame during a drag; bumping modified would reshuffle \
             the board on every mouse move"
        );
        assert!(store.is_dirty(), "the size must still reach disk on the next tick");
    }

    #[test]
    fn a_user_edit_still_moves_the_modified_timestamp() {
        // The other half of the rule above. `set_size` is a window event and
        // must not reorder the board; typing into a note is an edit and must.
        let mut store = temp_store("modified_split");
        let id = store.create("note".into(), NoteColor::Purple, NoteOrigin::Dictated);
        let before = store.get(&id).unwrap().modified.clone();

        store.set_size(&id, (400, 320));
        assert_eq!(store.get(&id).unwrap().modified, before);

        store.set_body(&id, "typed something".into());
        assert_ne!(
            store.get(&id).unwrap().modified,
            before,
            "an edit is what newest-first ordering on the board is for"
        );
    }

    #[test]
    fn set_size_with_an_unchanged_value_does_not_dirty_the_store() {
        let mut store = temp_store("size_noop");
        let id = store.create("note".into(), NoteColor::Purple, NoteOrigin::Dictated);
        store.set_size(&id, (400, 320));
        store.flush_if_dirty();

        store.set_size(&id, (400, 320));

        assert!(
            !store.is_dirty(),
            "Resized fires again when the window maps; a redundant write would notify \
             every subscriber for a fact that did not change"
        );
        store.set_size("nope", (10, 10));
        assert!(!store.is_dirty(), "a missing id is a no-op");
    }

    #[test]
    fn adding_an_attachment_writes_the_record_and_the_token_together() {
        let mut store = temp_store("add");
        let id = store.create("ring Sarah".into(), NoteColor::Purple, NoteOrigin::Dictated);

        store.add_attachment(&id, image("a1", "/home/berkley/deck.png"));

        let note = store.get(&id).unwrap();
        assert_eq!(note.body, "ring Sarah\n[[beamer:a1]]");
        assert_eq!(note.attachments.len(), 1);
        assert_eq!(blocks::referenced_ids(&note.body), vec!["a1"]);
        assert_eq!(
            note.raw, "ring Sarah",
            "raw is the verbatim transcript and never gains a token"
        );
    }

    #[test]
    fn removing_an_attachment_drops_the_record_and_the_token() {
        let mut store = temp_store("remove");
        let id = store.create("above".into(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(&id, image("a1", "/x.png"));
        store.set_body(&id, "above\n[[beamer:a1]]\nbelow".into());

        store.remove_attachment(&id, "a1");

        let note = store.get(&id).unwrap();
        assert_eq!(note.body, "above\nbelow", "the runs either side merge");
        assert!(note.attachments.is_empty());
    }

    #[test]
    fn prune_drops_a_record_whose_token_was_deleted_and_keeps_one_that_remains() {
        let mut store = temp_store("prune");
        let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(&id, image("keep", "/keep.png"));
        store.add_attachment(&id, image("gone", "/gone.png"));
        // The user selected the second token in the textarea and deleted it.
        store.set_body(&id, "[[beamer:keep]]".into());
        store.flush_if_dirty();

        assert!(store.prune_attachments(&id));

        let note = store.get(&id).unwrap();
        assert_eq!(note.attachments.len(), 1);
        assert_eq!(note.attachments[0].id(), "keep");
        assert!(store.is_dirty());
    }

    #[test]
    fn prune_with_nothing_orphaned_does_not_dirty_the_store() {
        let mut store = temp_store("prune_noop");
        let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(&id, image("a1", "/x.png"));
        store.flush_if_dirty();

        assert!(!store.prune_attachments(&id));
        assert!(
            !store.is_dirty(),
            "prune runs on parse and on save; a no-op must not schedule a write"
        );
        assert!(!store.prune_attachments("missing"));
    }

    #[test]
    fn relocate_repoints_a_moved_file_and_keeps_its_place_in_the_body() {
        let mut store = temp_store("relocate");
        let id = store.create("above".into(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(&id, image("a1", "/old/deck.png"));
        let body_before = store.get(&id).unwrap().body.clone();

        assert!(store.relocate_attachment(&id, "a1", PathBuf::from("/new/deck.png")));

        let note = store.get(&id).unwrap();
        assert_eq!(note.attachments[0].path().unwrap(), PathBuf::from("/new/deck.png"));
        assert_eq!(note.body, body_before, "relocating must not move the attachment");
        assert!(!store.relocate_attachment(&id, "a1", PathBuf::from("/new/deck.png")),
            "repointing at the same path changes nothing");
        assert!(!store.relocate_attachment(&id, "nope", PathBuf::from("/x")));
    }

    #[test]
    fn a_link_has_no_file_to_relocate() {
        let mut store = temp_store("relocate_link");
        let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(
            &id,
            Attachment::Link { id: "l1".into(), url: "https://example.com".into(), title: None },
        );
        assert!(!store.relocate_attachment(&id, "l1", PathBuf::from("/x.png")));
    }

    #[test]
    fn delete_removes_the_note_for_good() {
        let mut store = temp_store("delete");
        let keep = store.create("keep".into(), NoteColor::Purple, NoteOrigin::Dictated);
        let gone = store.create("gone".into(), NoteColor::Rose, NoteOrigin::Dictated);
        store.archive(&gone);
        store.flush_if_dirty();

        assert!(store.delete(&gone));

        assert!(store.get(&gone).is_none(), "delete is not archive");
        assert!(store.get(&keep).is_some());
        assert!(store.is_dirty());
        assert!(!store.delete(&gone), "deleting twice reports nothing was removed");
    }

    #[test]
    fn deleting_a_missing_note_does_not_dirty_the_store() {
        let mut store = temp_store("delete_missing");
        assert!(!store.delete("nope"));
        assert!(!store.is_dirty());
    }

    #[test]
    fn attachments_survive_a_round_trip_through_disk() {
        let mut store = temp_store("roundtrip");
        let id = store.create("look".into(), NoteColor::Amber, NoteOrigin::Dictated);
        store.add_attachment(&id, image("a1", "/home/berkley/pics/cat.png"));
        store.add_attachment(
            &id,
            Attachment::Link {
                id: "l1".into(),
                url: "https://figma.com/file/abc".into(),
                title: Some("Q3 deck".into()),
            },
        );
        store.flush_if_dirty();

        let text = std::fs::read_to_string(&store.path).unwrap();
        let reloaded: NoteStore = serde_json::from_str(&text).unwrap();
        let note = &reloaded.notes[0];

        assert_eq!(note.attachments.len(), 2);
        assert_eq!(note.attachments[0], image("a1", "/home/berkley/pics/cat.png"));
        assert_eq!(note.attachments[1].label(), "Q3 deck");
        assert_eq!(blocks::referenced_ids(&note.body), vec!["a1", "l1"]);
    }

    #[test]
    fn a_note_written_before_attachments_existed_loads_with_an_empty_vec() {
        // Migration stays free: no `deny_unknown_fields`, every new field
        // defaulted. Same mechanism as the stage fields.
        let json = r#"{
            "id": "18f2a1b3-0001",
            "created": "2026-08-01T09:15:00+01:00",
            "modified": "2026-08-01T09:15:00+01:00",
            "raw": "call the vet",
            "body": "call the vet",
            "color": "amber",
            "pos": null,
            "size": null,
            "open": true,
            "archived": false
        }"#;
        let note: Note = serde_json::from_str(json)
            .expect("an existing notes.json must keep loading");
        assert!(note.attachments.is_empty());
    }

    #[test]
    fn attachment_lookup_reports_a_desynchronised_note_rather_than_guessing() {
        let mut store = temp_store("lookup");
        let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(&id, image("a1", "/x.png"));
        let note = store.get(&id).unwrap();

        assert!(NoteStore::attachment(note, "a1").is_some());
        assert!(
            NoteStore::attachment(note, "ghost").is_none(),
            "an unmatched token renders as literal text — visibly wrong, not silently swallowed"
        );
    }

    #[test]
    fn searching_a_note_does_not_match_its_own_tokens() {
        let mut store = temp_store("search_tokens");
        let id = store.create("holiday photos".into(), NoteColor::Purple, NoteOrigin::Dictated);
        store.add_attachment(&id, image("a1", "/x.png"));

        assert!(
            store.search("beamer").is_empty(),
            "without plain_text every attachment-bearing note would match its own token"
        );
        assert_eq!(store.search("holiday").len(), 1);
        let _ = id;
    }
}
