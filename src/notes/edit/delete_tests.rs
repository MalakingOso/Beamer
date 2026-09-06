//! Delete tests for [`super`].
//!
//! A rule carried over from `tests.rs`: **no test ever names a path that
//! could be a real file on the machine running it.** Every path below is
//! either `/nonexistent/…` (deliberately unreadable) or a file this module
//! wrote itself under `std::env::temp_dir()`.


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
