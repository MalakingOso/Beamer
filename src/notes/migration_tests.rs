//! Migration tests for `mod.rs`: the pre-document load path, which is what
//! every test here exercises — no `notes.automerge` exists, so the store
//! seeds itself from `notes.json` exactly as an install upgrading to the
//! document era does.

use std::path::PathBuf;

use super::*;

/// The pre-document load path, which is what every migration test here
/// exercises: no `notes.automerge` exists, so the store seeds itself from
/// `notes.json` exactly as an install upgrading to this task does. The
/// document path is derived from the notes path and is never written by
/// these tests.
fn seed_load(path: PathBuf, machine_path: PathBuf) -> NoteStore {
    let doc_path = path.with_extension("automerge");
    let _ = std::fs::remove_file(&doc_path);
    NoteStore::load_from(path, machine_path, doc_path)
}

/// PID-scoped temp path so concurrent test runs don't race and nothing
/// touches the real user config dir. Mirrors `ui::history`'s tests.
///
/// `machine.json` gets its own file alongside `notes.json`, named from
/// the same tag, so tests in this module never share one machine store.
/// A shared one would let two tests running in parallel race on it.

/// A `notes.json` shaped exactly as a pre-Task-7 install would have
/// written it: two notes, each carrying `pos`/`size`/`open` inline.
/// Structure copied from the real schema with invented note text, never
/// the user's actual notes.
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
fn a_note_loaded_from_a_legacy_file_no_longer_carries_pos_size_or_open_itself() {
    let (path, machine_path) = temp_migration_paths("migrate_shape");

    let store = seed_load(path, machine_path);

    // The old keys are simply ignored by `Note`'s own deserialize (no
    // `deny_unknown_fields`, so this must not be fatal), and the content
    // fields must still be intact.
    let note = store.get("199012340-0000").unwrap();
    assert_eq!(note.raw, "call the vet about biscuit");
    assert_eq!(note.color, NoteColor::Amber);
    assert!(!note.archived);
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

/// GC only ever runs against a successfully parsed `notes.json`. A missing,
/// unreadable or corrupt (quarantined) file tells us nothing about which
/// notes exist, and must not be read as "no notes exist". That would wipe
/// `machine.json` permanently on what may be a transient read error, which
/// stops being theoretical once a sync writer can be mid-replace of
/// `notes.json` when a load lands.
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
fn a_load_with_nothing_to_migrate_does_not_dirty_the_store() {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("no_migration.json");
    let machine_path = dir.join("no_migration.machine.json");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&machine_path);

    let mut store = NoteStore {
        notes: Vec::new(),
        path: path.clone(),
        dirty: false,
        machine: MachineStore::new(machine_path.clone()),
        doc: sync_doc::SyncHandle::default(),
        doc_dirty: false,
        load_error: None,
        unreadable_notes: Vec::new(),
    };
    store.create("hello".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.flush_if_dirty();

    let reloaded = seed_load(path, machine_path);

    let reason = "a notes.json already written under the current schema carries no \
                   legacy keys, so there is nothing to migrate and nothing to rewrite";
    assert!(!reloaded.is_dirty(), "{}", reason);
}

#[test]
fn a_notes_json_from_before_attachments_were_removed_still_loads() {
    // The `attachments` key is simply ignored by `Note`'s own deserialize
    // (no `deny_unknown_fields`), so a file written while attachments
    // existed loads with its text intact and the stale key dropped on the
    // next flush. Same mechanism as the stage fields.
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
