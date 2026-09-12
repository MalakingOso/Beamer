//! Tests for [`super`]. Split into their own file for the same reason
//! `task_store/tests.rs` and `pipeline/tests.rs` are: the parent module was
//! closing in on the project's 500-line limit.

use super::*;
use crate::notes::{NoteColor, NoteOrigin};

fn temp_store(tag: &str) -> NoteStore {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("edit_{tag}.json"));
    let machine_path = dir.join(format!("edit_{tag}.machine.json"));
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
