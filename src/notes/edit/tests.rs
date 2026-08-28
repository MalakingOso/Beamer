//! Tests for [`super`].
//!
//! Split into their own file for the same reason `task_store/tests.rs` and
//! `pipeline/tests.rs` were: this task's copy-on-attach and refcounting logic
//! pushed `edit.rs` well past room for its own tests. Dedented one level,
//! same suite.
//!
//! A rule specific to this file: **no test ever names a path that could be a
//! real file on the machine running it.** `add_attachment` and
//! `relocate_attachment` now actually try to read whatever path an
//! `Attachment` carries, so a path that happens to exist would get its real
//! bytes copied into a test's temp `attachments_dir`, harmless to the
//! original, but a test must never depend on what is or is not on someone's
//! disk. Every path below is either `/nonexistent/…` (deliberately unreadable,
//! for the tests that are really about the pre-migration `External` shape) or
//! a file this module wrote itself under `std::env::temp_dir()` (for the tests
//! that are really about copying bytes in).

use std::path::PathBuf;

use super::*;
use crate::notes::{NoteColor, NoteOrigin};

fn temp_store(tag: &str) -> NoteStore {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("edit_{tag}.json"));
    let machine_path = dir.join(format!("edit_{tag}.machine.json"));
    let attachments_dir = dir.join(format!("edit_{tag}_attachments"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&machine_path);
    let _ = std::fs::remove_dir_all(&attachments_dir);
    NoteStore {
        notes: Vec::new(),
        path,
        dirty: false,
        machine: crate::notes::MachineStore::new(machine_path),
        attachments_dir,
        doc: crate::notes::sync_doc::SyncHandle::default(),
        doc_dirty: false,
        load_error: None,
        unreadable_notes: Vec::new(),
        sync_enabled: false,
    }
}

/// An `Image` attachment pointing at `path`, in the pre-migration `External`
/// shape: exactly what a fresh drop looks like before `add_attachment` has
/// had a chance to try adopting it.
fn image(id: &str, path: &str) -> Attachment {
    let path = PathBuf::from(path);
    let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Attachment::Image { id: id.into(), filename, alt: None, location: Location::External { path } }
}

/// A real file under this module's own temp scratch space, distinct from any
/// store's `attachments_dir`, so a "copy on attach" test has real bytes to
/// hash without ever touching a directory a store also writes to.
fn temp_source(tag: &str, name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}_sources", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{tag}_{name}"));
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn set_size_records_the_size_without_moving_the_modified_timestamp() {
    let mut store = temp_store("size");
    let id = store.create("note".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let before = store.get(&id).unwrap().modified.clone();

    store.set_size(&id, (400, 320));

    assert_eq!(store.size(&id), Some((400, 320)));
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

    store.add_attachment(&id, image("a1", "/nonexistent/deck.png"));

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
fn attaching_an_unreadable_path_keeps_it_external_rather_than_failing() {
    let mut store = temp_store("add_unreadable");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);

    store.add_attachment(&id, image("a1", "/nonexistent/deck.png"));

    let note = store.get(&id).unwrap();
    assert_eq!(
        note.attachments[0].location(),
        Some(&Location::External { path: PathBuf::from("/nonexistent/deck.png") }),
        "a file Beamer cannot read at attach time is stored as-is, not dropped"
    );
}

#[test]
fn attaching_a_readable_path_copies_the_bytes_in() {
    let mut store = temp_store("add_readable");
    let source = temp_source("add_readable", "deck.png", b"a real photo, allegedly");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);

    store.add_attachment(&id, image("a1", &source.to_string_lossy()));

    let note = store.get(&id).unwrap();
    match note.attachments[0].location() {
        Some(Location::Owned { hash, ext }) => {
            assert_eq!(ext, "png");
            let copy = store.attachments_dir.join(format!("{hash}.{ext}"));
            assert!(copy.exists(), "copy on attach must actually copy");
            assert_eq!(std::fs::read(&copy).unwrap(), b"a real photo, allegedly");
        }
        other => panic!("expected an owned attachment, got {other:?}"),
    }
    assert!(source.exists(), "attaching copies the file, it never moves or deletes the original");
}

#[test]
fn the_same_bytes_attached_twice_yield_one_file_and_one_hash() {
    let mut store = temp_store("dedupe");
    let source = temp_source("dedupe", "cat.png", b"identical bytes, two attachments");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);

    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    store.add_attachment(&id, image("a2", &source.to_string_lossy()));

    let note = store.get(&id).unwrap();
    let hash_of = |a: &Attachment| owned_hash(a).expect("both attachments must have been adopted").0;
    assert_eq!(
        hash_of(&note.attachments[0]),
        hash_of(&note.attachments[1]),
        "identical bytes must hash to the same content address"
    );

    let files_written: usize = std::fs::read_dir(&store.attachments_dir).unwrap().count();
    assert_eq!(files_written, 1, "one file must serve both attachments");
}

#[test]
fn removing_an_attachment_drops_the_record_and_the_token() {
    let mut store = temp_store("remove");
    let id = store.create("above".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", "/nonexistent/x.png"));
    store.set_body(&id, "above\n[[beamer:a1]]\nbelow".into());

    store.remove_attachment(&id, "a1");

    let note = store.get(&id).unwrap();
    assert_eq!(note.body, "above\nbelow", "the runs either side merge");
    assert!(note.attachments.is_empty());
}

#[test]
fn removing_the_last_reference_to_a_hash_removes_its_file() {
    let mut store = temp_store("remove_release");
    let source = temp_source("remove_release", "deck.png", b"only one attachment uses this");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    let (hash, ext) = owned_hash(&store.get(&id).unwrap().attachments[0]).unwrap();
    let file = store.attachments_dir.join(format!("{hash}.{ext}"));
    assert!(file.exists());

    store.remove_attachment(&id, "a1");

    assert!(!file.exists(), "nothing references this hash any more");
}

#[test]
fn prune_drops_a_record_whose_token_was_deleted_and_keeps_one_that_remains() {
    let mut store = temp_store("prune");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("keep", "/nonexistent/keep.png"));
    store.add_attachment(&id, image("gone", "/nonexistent/gone.png"));
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
    store.add_attachment(&id, image("a1", "/nonexistent/x.png"));
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
    store.add_attachment(&id, image("a1", "/nonexistent/old/deck.png"));
    let body_before = store.get(&id).unwrap().body.clone();

    assert!(store.relocate_attachment(&id, "a1", PathBuf::from("/nonexistent/new/deck.png")));

    let note = store.get(&id).unwrap();
    assert_eq!(
        note.attachments[0].location(),
        Some(&Location::External { path: PathBuf::from("/nonexistent/new/deck.png") }),
        "a path Beamer still cannot read stays External"
    );
    assert_eq!(note.body, body_before, "relocating must not move the attachment");
    assert!(
        !store.relocate_attachment(&id, "a1", PathBuf::from("/nonexistent/new/deck.png")),
        "repointing at the same path changes nothing"
    );
    assert!(!store.relocate_attachment(&id, "nope", PathBuf::from("/nonexistent/x")));
}

#[test]
fn relocating_at_a_readable_path_adopts_it_and_releases_the_old_hash() {
    let mut store = temp_store("relocate_adopt");
    let old_source = temp_source("relocate_adopt", "old.png", b"the file that used to be here");
    let new_source = temp_source("relocate_adopt", "new.png", b"the file the user just picked");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &old_source.to_string_lossy()));
    let (old_hash, old_ext) = owned_hash(&store.get(&id).unwrap().attachments[0]).unwrap();
    let old_file = store.attachments_dir.join(format!("{old_hash}.{old_ext}"));
    assert!(old_file.exists());

    assert!(store.relocate_attachment(&id, "a1", new_source.clone()));

    let note = store.get(&id).unwrap();
    match note.attachments[0].location() {
        Some(Location::Owned { hash, .. }) => assert_ne!(hash, &old_hash),
        other => panic!("expected the new file to be adopted, got {other:?}"),
    }
    assert!(!old_file.exists(), "nothing references the old hash any more");
    assert!(old_source.exists(), "relocating never touches either original file");
    assert!(new_source.exists());
}

#[test]
fn a_link_has_no_file_to_relocate() {
    let mut store = temp_store("relocate_link");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(
        &id,
        Attachment::Link { id: "l1".into(), url: "https://example.com".into(), title: None },
    );
    assert!(!store.relocate_attachment(&id, "l1", PathBuf::from("/nonexistent/x.png")));
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
fn delete_also_drops_the_note_s_machine_local_window_state() {
    let mut store = temp_store("delete_gc");
    let id = store.create("gone".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.set_size(&id, (400, 300));
    assert!(store.is_open(&id));

    store.delete(&id);

    assert_eq!(store.size(&id), None, "a deleted note's window state must not linger");
    assert!(!store.is_open(&id));
}

#[test]
fn deleting_one_of_two_notes_sharing_a_hash_keeps_the_file() {
    let mut store = temp_store("shared_keep");
    let source = temp_source("shared_keep", "deck.png", b"shared between two notes");
    let a = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    let b = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&a, image("a1", &source.to_string_lossy()));
    store.add_attachment(&b, image("b1", &source.to_string_lossy()));
    let (hash, ext) = owned_hash(&store.get(&a).unwrap().attachments[0]).unwrap();
    let file = store.attachments_dir.join(format!("{hash}.{ext}"));
    assert!(file.exists());

    store.delete(&a);

    assert!(file.exists(), "note b still references this hash");
}

#[test]
fn deleting_the_last_note_referencing_a_hash_removes_the_file() {
    let mut store = temp_store("shared_remove");
    let source = temp_source("shared_remove", "deck.png", b"only one reference left");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    let (hash, ext) = owned_hash(&store.get(&id).unwrap().attachments[0]).unwrap();
    let file = store.attachments_dir.join(format!("{hash}.{ext}"));
    assert!(file.exists());

    store.delete(&id);

    assert!(!file.exists(), "nothing left references this hash");
}

#[test]
fn with_sync_configured_deleting_the_last_reference_keeps_the_file() {
    // Same shape as `deleting_the_last_note_referencing_a_hash_removes_the_file`,
    // sync on: the only difference this flag is allowed to make.
    let mut store = temp_store("sync_keep_delete");
    store.sync_enabled = true;
    let source = temp_source("sync_keep_delete", "deck.png", b"a peer machine might still need this");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    let (hash, ext) = owned_hash(&store.get(&id).unwrap().attachments[0]).unwrap();
    let file = store.attachments_dir.join(format!("{hash}.{ext}"));
    assert!(file.exists());

    store.delete(&id);

    assert!(
        file.exists(),
        "with sync configured, this store's own refcount reaching zero is not proof \
         nothing elsewhere still needs the bytes"
    );
    assert!(source.exists(), "the user's original is untouched either way");
}

#[test]
fn with_sync_configured_removing_the_last_reference_to_an_attachment_keeps_the_file() {
    // The `remove_attachment` counterpart to the test above: removing a
    // single attachment (not deleting the whole note) is refcounted the
    // same way, through the same `release_attachment_bytes`.
    let mut store = temp_store("sync_keep_remove");
    store.sync_enabled = true;
    let source = temp_source("sync_keep_remove", "deck.png", b"kept in case a peer still points at it");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    let (hash, ext) = owned_hash(&store.get(&id).unwrap().attachments[0]).unwrap();
    let file = store.attachments_dir.join(format!("{hash}.{ext}"));

    store.remove_attachment(&id, "a1");

    assert!(file.exists(), "with sync on, an unreferenced file is kept rather than deleted");
    assert!(source.exists());
}

#[test]
fn without_sync_configured_the_refcounted_delete_still_removes_the_file() {
    // Pins the default: a store built without ever calling
    // `set_sync_enabled` behaves exactly as it always has. `sync_enabled`
    // defaulting to anything else would silently change every existing
    // install's behaviour the moment this field was added.
    let mut store = temp_store("no_sync_default");
    assert!(!store.sync_enabled, "sync must default to off");
    let source = temp_source("no_sync_default", "deck.png", b"nothing else on this machine wants this");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    let (hash, ext) = owned_hash(&store.get(&id).unwrap().attachments[0]).unwrap();
    let file = store.attachments_dir.join(format!("{hash}.{ext}"));

    store.delete(&id);

    assert!(!file.exists(), "sync off is the existing behaviour, unchanged");
}

#[test]
fn deleting_a_note_never_touches_the_original_source_file() {
    let mut store = temp_store("source_untouched");
    let source = temp_source("source_untouched", "deck.png", b"do not touch me, ever");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", &source.to_string_lossy()));
    assert!(source.exists());

    store.delete(&id);

    assert!(
        source.exists(),
        "delete must remove Beamer's own copy, never the file it was copied from"
    );
}

#[test]
fn deleting_a_note_never_touches_an_unmigrated_external_path_either() {
    // The `Owned` case above goes through the copy-and-hash path; this one
    // never gets that far. `External`'s whole record *is* the user's own
    // path, and it is exactly the shape a future refactor of `owned_hash`
    // (which only ever matches `Owned`) could accidentally start deleting
    // through if that guard were loosened. Built by pushing the attachment
    // directly rather than through `add_attachment`, standing in for a note
    // whose attachment is legitimately still `External` at delete time (not
    // yet migrated, or arrived from another machine) with nothing about
    // readability involved.
    let mut store = temp_store("source_untouched_external");
    let source = temp_source("source_untouched_external", "deck.png", b"do not touch me, ever");
    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    let attachment = Attachment::Image {
        id: "a1".into(),
        filename: "deck.png".into(),
        alt: None,
        location: Location::External { path: source.clone() },
    };
    store.notes.iter_mut().find(|n| n.id == id).unwrap().attachments.push(attachment);
    assert!(source.exists());

    store.delete(&id);

    assert!(
        source.exists(),
        "delete must never touch the path an External attachment's record carries,          adopted or not"
    );
}

#[test]
fn a_traversal_shaped_hash_is_never_used_to_remove_a_file_outside_attachments_dir() {
    // Standing in for an attachment that arrived already `Owned` from a
    // hand-edited, or (once notes sync) maliciously crafted, `notes.json`:
    // never one this process adopted itself, since `adopt_into` only ever
    // produces a valid 64-hex-digit hash.
    let mut store = temp_store("traversal");
    let sentinel_dir = store.attachments_dir.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&sentinel_dir).unwrap();
    let sentinel = sentinel_dir.join("traversal_sentinel.png");
    std::fs::write(&sentinel, b"do not delete me").unwrap();

    let id = store.create(String::new(), NoteColor::Purple, NoteOrigin::Dictated);
    let malicious = Attachment::Image {
        id: "a1".into(),
        filename: "cat.png".into(),
        alt: None,
        location: Location::Owned { hash: "../traversal_sentinel".into(), ext: "png".into() },
    };
    store.notes.iter_mut().find(|n| n.id == id).unwrap().attachments.push(malicious);

    store.delete(&id);

    assert!(
        sentinel.exists(),
        "an unrecognised hash/ext must never let deletion reach outside attachments_dir"
    );
}

#[test]
fn attachments_survive_a_round_trip_through_disk() {
    let mut store = temp_store("roundtrip");
    let id = store.create("look".into(), NoteColor::Amber, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", "/nonexistent/pics/cat.png"));
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
    assert_eq!(note.attachments[0], image("a1", "/nonexistent/pics/cat.png"));
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
    store.add_attachment(&id, image("a1", "/nonexistent/x.png"));
    let note = store.get(&id).unwrap();

    assert!(NoteStore::attachment(note, "a1").is_some());
    assert!(
        NoteStore::attachment(note, "ghost").is_none(),
        "an unmatched token renders as literal text, visibly wrong, not silently swallowed"
    );
}

#[test]
fn searching_a_note_does_not_match_its_own_tokens() {
    let mut store = temp_store("search_tokens");
    let id = store.create("holiday photos".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.add_attachment(&id, image("a1", "/nonexistent/x.png"));

    assert!(
        store.search("beamer").is_empty(),
        "without plain_text every attachment-bearing note would match its own token"
    );
    assert_eq!(store.search("holiday").len(), 1);
    let _ = id;
}
