//! Migration tests for `NoteStore`'s pre-document load path (`legacy.rs`): no
//! `notes.automerge` exists, so the store seeds itself from `notes.json` and
//! lifts window state into `machine.json`, as an upgrading install does.

use std::path::PathBuf;

use super::*;

/// Load with no `notes.automerge` present, forcing the seed path. The document
/// path is derived from the notes path and is never written by these tests.
fn seed_load(path: PathBuf, machine_path: PathBuf) -> NoteStore {
    let doc_path = path.with_extension("automerge");
    let _ = std::fs::remove_file(&doc_path);
    NoteStore::load_from(path, machine_path, doc_path)
}

/// A `notes.json` from before `machine.json` existed: two notes, each carrying
/// `pos`/`size`/`open` inline. Real schema, invented text.
const LEGACY_NOTES_JSON: &str = r#"{
    "notes": [
        {
            "id": "199012340-0000",
            "created": "2026-08-01T09:15:00+01:00",
            "modified": "2026-08-01T09:15:00+01:00",
            "raw": "call the vet about biscuit",
            "body": "call the vet about biscuit",
            "clean_state": "pending",
            "extract_state": "pending",
            "origin": "dictated",
            "color": "amber",
            "pos": [100, 200],
            "size": [320, 240],
            "open": true,
            "archived": false
        },
        {
            "id": "199012340-0001",
            "created": "2026-08-01T09:16:00+01:00",
            "modified": "2026-08-01T09:16:00+01:00",
            "raw": "send the invoice",
            "body": "send the invoice",
            "clean_state": "pending",
            "extract_state": "pending",
            "origin": "typed",
            "color": "teal",
            "pos": null,
            "size": null,
            "open": false,
            "archived": true
        }
    ]
}"#;

/// PID-scoped `notes.json` (holding the legacy fixture) plus its own
/// `machine.json`, both named by `tag`: parallel tests must never share a
/// machine store, and nothing may touch the real config dir.
fn temp_migration_paths(tag: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{tag}.json"));
    let machine_path = dir.join(format!("{tag}.machine.json"));
    std::fs::write(&path, LEGACY_NOTES_JSON).unwrap();
    let _ = std::fs::remove_file(&machine_path);
    (path, machine_path)
}

#[test]
fn migration_lifts_pos_size_and_open_off_a_legacy_notes_json_losslessly() {
    let (path, machine_path) = temp_migration_paths("migrate");

    let store = seed_load(path, machine_path);

    assert_eq!(store.notes.len(), 2, "the notes themselves must still load");
    assert_eq!(store.pos("199012340-0000"), Some((100, 200)));
    assert_eq!(store.size("199012340-0000"), Some((320, 240)));
    assert!(store.is_open("199012340-0000"));

    assert_eq!(store.pos("199012340-0001"), None);
    assert_eq!(store.size("199012340-0001"), None);
    assert!(!store.is_open("199012340-0001"));
}

#[test]
fn loading_gcs_machine_entries_for_notes_that_no_longer_exist() {
    let (path, machine_path) = temp_migration_paths("gc_on_load");
    // Simulate a stale machine.json left over from a note that was since
    // deleted from notes.json by hand (or on another machine, synced).
    {
        let mut machine = MachineStore::new(machine_path.clone());
        machine.set_open("199012340-0000", true);
        machine.set_open("long-gone", true);
        machine.flush_if_dirty();
    }

    let store = seed_load(path, machine_path);

    assert!(store.is_open("199012340-0000"), "a note still present keeps its state");
    assert!(!store.is_open("long-gone"), "an entry for a note that no longer exists must be dropped");
}

/// GC only runs against a successfully parsed `notes.json`. A missing,
/// unreadable or quarantined file says nothing about which notes exist;
/// reading it as "none" would wipe `machine.json` on a transient read error.
#[test]
fn a_missing_notes_json_leaves_an_existing_machine_json_intact() {
    let (path, machine_path) = temp_migration_paths("missing_notes");
    std::fs::remove_file(&path).unwrap();
    {
        let mut machine = MachineStore::new(machine_path.clone());
        machine.set_open("still-here", true);
        machine.set_size("still-here", (400, 300));
        machine.flush_if_dirty();
    }

    let store = seed_load(path, machine_path);

    assert!(
        store.is_open("still-here"),
        "a missing notes.json is a read failure, not proof the note is gone"
    );
    assert_eq!(store.size("still-here"), Some((400, 300)));
}

#[test]
fn a_quarantined_corrupt_notes_json_leaves_an_existing_machine_json_intact() {
    let (path, machine_path) = temp_migration_paths("corrupt_notes");
    std::fs::write(&path, "not valid json").unwrap();
    {
        let mut machine = MachineStore::new(machine_path.clone());
        machine.set_open("still-here", true);
        machine.flush_if_dirty();
    }

    let store = seed_load(path, machine_path);

    assert!(
        store.is_open("still-here"),
        "quarantining a corrupt notes.json must not also wipe machine.json"
    );
}

#[test]
fn a_migrating_load_leaves_the_store_dirty_so_the_stale_keys_get_rewritten_away() {
    let (path, machine_path) = temp_migration_paths("migrate_dirty");

    let store = seed_load(path, machine_path);

    assert!(
        store.is_dirty(),
        "lifting legacy fields off notes.json must dirty the store, or a          never-edited legacy file could sync to a second machine and leak          the first machine's window state into that machine's own migration"
    );
}

#[test]
fn a_notes_json_from_before_attachments_were_removed_still_loads() {
    // `attachments` is ignored like any unknown key, so the text loads intact
    // and the stale key is dropped on the next flush.
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pre_removal.json");
    let machine_path = dir.join("pre_removal.machine.json");
    let _ = std::fs::remove_file(&machine_path);
    std::fs::write(
        &path,
        r#"{
            "notes": [{
                "id": "18f2a1b3-0002",
                "created": "2026-08-01T09:15:00+01:00",
                "modified": "2026-08-01T09:15:00+01:00",
                "raw": "look at this",
                "body": "look at this",
                "color": "amber",
                "attachments": [
                    {"kind": "image", "id": "a1", "filename": "cat.png",
                     "alt": null, "location": {"kind": "owned", "hash": "b", "ext": "png"}}
                ],
                "archived": false
            }]
        }"#,
    )
    .unwrap();

    let store = seed_load(path, machine_path);

    let note = &store.notes[0];
    assert_eq!(note.raw, "look at this");
    assert_eq!(note.body, "look at this");
}
