//! Delete tests for [`super`].

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
