//! Tests for `mod.rs`, split out under `#[path]` for the same reason
//! `task_store/tests.rs` and `pipeline/tests.rs` are: `mod.rs` was closing in
//! on the project's 500-line limit and this task added a machine-id scheme
//! and a migration path, each of which wants its own tests.

use super::*;

/// PID-scoped temp path so concurrent test runs don't race and nothing
/// touches the real user config dir. Mirrors `ui::history`'s tests.
///
/// `machine.json` gets its own file alongside `notes.json`, named from
/// the same tag, so tests in this module never share one machine store.
/// A shared one would let two tests running in parallel race on it.
fn temp_store(tag: &str) -> NoteStore {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{tag}.json"));
    let machine_path = dir.join(format!("{tag}.machine.json"));
    let attachments_dir = dir.join(format!("{tag}_attachments"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&machine_path);
    let _ = std::fs::remove_dir_all(&attachments_dir);
    NoteStore { notes: Vec::new(), path, dirty: false, machine: MachineStore::new(machine_path), attachments_dir }
}

#[test]
fn create_returns_a_unique_id_and_seeds_body_from_raw() {
    let mut store = temp_store("create");
    let a = store.create("call the vet".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let b = store.create("send invoice".into(), NoteColor::Teal, NoteOrigin::Dictated);

    assert_ne!(a, b, "ids must be unique even within the same millisecond");

    let note = store.get(&a).unwrap();
    assert_eq!(note.raw, "call the vet");
    assert_eq!(note.body, "call the vet", "body starts as a copy of raw");
    assert_eq!(note.clean_state, StageState::Pending);
    assert_eq!(note.extract_state, StageState::Pending);
    assert_eq!(
        note.origin, NoteOrigin::Dictated,
        "a note created by the capture path is dictated; mislabelling it corrupts provenance"
    );
    assert!(!note.archived);
}

#[test]
fn set_body_never_touches_raw() {
    let mut store = temp_store("raw_immutable");
    let id = store.create("um so call the vet".into(), NoteColor::Purple, NoteOrigin::Dictated);

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
    let keep = store.create("keep".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let gone = store.create("archive me".into(), NoteColor::Rose, NoteOrigin::Dictated);

    store.archive(&gone);

    let active: Vec<&str> = store.active().iter().map(|n| n.raw.as_str()).collect();
    assert_eq!(active, vec!["keep"]);
    assert!(store.get(&gone).is_some(), "archiving must not delete");
    let _ = keep;
}

#[test]
fn flush_writes_only_when_dirty() {
    let mut store = temp_store("debounce");
    store.create("something".into(), NoteColor::Purple, NoteOrigin::Dictated);

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
    store.create("hello".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.flush_if_dirty();

    assert!(!store.path.with_extension("json.tmp").exists());
}

#[test]
fn notes_round_trip_through_disk() {
    // `pos` and `size` used to be asserted here too, back when they were
    // fields on `Note` itself. They round-trip through `machine.json` now
    // instead (see `machine::tests`), and `notes.json` no longer carries
    // them at all, which this test also pins. `serde_json::to_string`
    // must not mention them.
    let mut store = temp_store("roundtrip");
    store.create("first".into(), NoteColor::Amber, NoteOrigin::Dictated);
    store.flush_if_dirty();

    let text = std::fs::read_to_string(&store.path).unwrap();
    assert!(!text.contains("\"pos\""), "pos must not live in notes.json any more");
    assert!(!text.contains("\"size\""), "size must not live in notes.json any more");
    assert!(!text.contains("\"open\""), "open must not live in notes.json any more");

    let reloaded: NoteStore = serde_json::from_str(&text).unwrap();
    assert_eq!(reloaded.notes.len(), 1);
    assert_eq!(reloaded.notes[0].color, NoteColor::Amber);
}

#[test]
fn set_open_with_an_unchanged_value_does_not_dirty_the_store() {
    let mut store = temp_store("set_open");
    let id = store.create("hello".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.flush_if_dirty();
    let before = store.get(&id).unwrap().modified.clone();

    store.set_open(&id, true); // already true

    assert!(
        !store.is_dirty(),
        "a redundant set_open must not schedule a write, the callers are \
         window events, not user edits"
    );
    assert_eq!(
        store.get(&id).unwrap().modified,
        before,
        "nothing changed, so the modified timestamp must not move"
    );

    store.set_open(&id, false);
    assert!(store.is_dirty(), "a real change must still be persisted");
    assert!(!store.is_open(&id));
}

#[test]
fn set_open_does_not_move_the_modified_timestamp_even_on_a_real_change() {
    // The stronger claim than the unchanged-value test above: `set_open`
    // must never touch `modified`, not even when it does change the
    // value. This is the exact scenario the brief calls out. Closing a
    // sticky must not be able to outrank a real edit under a
    // last-write-wins sync merge.
    let mut store = temp_store("set_open_modified");
    let id = store.create("hello".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let before = store.get(&id).unwrap().modified.clone();

    store.set_open(&id, false);

    assert_eq!(store.get(&id).unwrap().modified, before);
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
    let id = store.create("um so call the vet about biscuit".into(), NoteColor::Purple, NoteOrigin::Dictated);
    // A cleanup pass rewrote the body and dropped the filler word.
    store.set_body(&id, "Call the vet about Biscuit.".into());

    assert_eq!(store.search("biscuit").len(), 1, "matching must be case-insensitive");
    assert_eq!(store.search("VET").len(), 1);
    assert_eq!(
        store.search("um so").len(),
        1,
        "raw is searched too, cleanup can remove the very words you remember saying"
    );
    assert!(store.search("mortgage").is_empty());
}

#[test]
fn an_empty_query_returns_every_active_note() {
    let mut store = temp_store("search_empty");
    store.create("one".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let gone = store.create("two".into(), NoteColor::Teal, NoteOrigin::Dictated);
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
    let id = store.create("bring me back".into(), NoteColor::Rose, NoteOrigin::Dictated);
    store.archive(&id);
    assert_eq!(store.archived().len(), 1);
    assert!(store.active().is_empty());

    store.restore(&id);

    assert!(store.active().iter().any(|n| n.id == id));
    assert!(store.archived().is_empty());
    assert!(
        !store.is_open(&id),
        "restoring puts a note back on the board; it does not pop a window open"
    );
}

#[test]
fn restoring_a_note_that_is_not_archived_does_nothing() {
    let mut store = temp_store("restore_noop");
    let id = store.create("already here".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.flush_if_dirty();

    store.restore(&id);

    assert!(!store.is_dirty(), "a no-op restore must not schedule a write");
}

#[test]
fn two_machine_suffixes_at_the_same_millis_and_counter_produce_different_ids() {
    assert_ne!(
        format_note_id(0x1990_1234, 0, "aaaa"),
        format_note_id(0x1990_1234, 0, "bbbb"),
        "two machines minting their first note at the same millisecond must not collide"
    );
}

#[test]
fn a_note_id_carries_its_machine_suffix() {
    let id = format_note_id(0x1990_1234, 3, "cafe");
    assert!(id.ends_with("-cafe"));
    assert_eq!(id, "19901234-0003-cafe");
}

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
            "attachments": [],
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
            "attachments": [],
            "open": false,
            "archived": true
        }
    ]
}"#;

fn temp_migration_paths(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{tag}.json"));
    let machine_path = dir.join(format!("{tag}.machine.json"));
    let attachments_dir = dir.join(format!("{tag}_attachments"));
    std::fs::write(&path, LEGACY_NOTES_JSON).unwrap();
    let _ = std::fs::remove_file(&machine_path);
    let _ = std::fs::remove_dir_all(&attachments_dir);
    (path, machine_path, attachments_dir)
}

#[test]
fn migration_lifts_pos_size_and_open_off_a_legacy_notes_json_losslessly() {
    let (path, machine_path, attachments_dir) = temp_migration_paths("migrate");

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

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
    let (path, machine_path, attachments_dir) = temp_migration_paths("migrate_shape");

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

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
    let (path, machine_path, attachments_dir) = temp_migration_paths("gc_on_load");
    // Simulate a stale machine.json left over from a note that was since
    // deleted from notes.json by hand (or on another machine, synced).
    {
        let mut machine = MachineStore::new(machine_path.clone());
        machine.set_open("199012340-0000", true);
        machine.set_open("long-gone", true);
        machine.flush_if_dirty();
    }

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

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
    let (path, machine_path, attachments_dir) = temp_migration_paths("missing_notes");
    std::fs::remove_file(&path).unwrap();
    {
        let mut machine = MachineStore::new(machine_path.clone());
        machine.set_open("still-here", true);
        machine.set_size("still-here", (400, 300));
        machine.flush_if_dirty();
    }

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

    assert!(
        store.is_open("still-here"),
        "a missing notes.json is a read failure, not proof the note is gone"
    );
    assert_eq!(store.size("still-here"), Some((400, 300)));
}

#[test]
fn a_quarantined_corrupt_notes_json_leaves_an_existing_machine_json_intact() {
    let (path, machine_path, attachments_dir) = temp_migration_paths("corrupt_notes");
    std::fs::write(&path, "not valid json").unwrap();
    {
        let mut machine = MachineStore::new(machine_path.clone());
        machine.set_open("still-here", true);
        machine.flush_if_dirty();
    }

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

    assert!(
        store.is_open("still-here"),
        "quarantining a corrupt notes.json must not also wipe machine.json"
    );
}

#[test]
fn a_migrating_load_leaves_the_store_dirty_so_the_stale_keys_get_rewritten_away() {
    let (path, machine_path, attachments_dir) = temp_migration_paths("migrate_dirty");

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

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
    let attachments_dir = dir.join("no_migration_attachments");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&machine_path);
    let _ = std::fs::remove_dir_all(&attachments_dir);

    let mut store = NoteStore {
        notes: Vec::new(),
        path: path.clone(),
        dirty: false,
        machine: MachineStore::new(machine_path.clone()),
        attachments_dir: attachments_dir.clone(),
    };
    store.create("hello".into(), NoteColor::Purple, NoteOrigin::Dictated);
    store.flush_if_dirty();

    let reloaded = NoteStore::load_from(path, machine_path, attachments_dir);

    let reason = "a notes.json already written under the current schema carries no \
                   legacy keys, so there is nothing to migrate and nothing to rewrite";
    assert!(!reloaded.is_dirty(), "{}", reason);
}

/// A single note carrying one attachment in the pre-Task-8 shape: `path`
/// directly on the object, no `filename` and no `location`. `__PATH__` is
/// substituted per-test, since one test's path exists on disk and the
/// other's deliberately does not.
const LEGACY_NOTES_WITH_ATTACHMENT_JSON: &str = r#"{
    "notes": [
        {
            "id": "18f2a1b3-0002",
            "created": "2026-08-01T09:15:00+01:00",
            "modified": "2026-08-01T09:15:00+01:00",
            "raw": "look at this",
            "body": "look at this\n[[beamer:a1]]",
            "color": "amber",
            "attachments": [
                {"kind": "image", "id": "a1", "path": "__PATH__", "alt": null}
            ],
            "pos": null,
            "size": null,
            "open": true,
            "archived": false
        }
    ]
}"#;

fn write_legacy_attachment_json(path: &Path, referenced: &Path) {
    // `to_string_lossy` plus a manual backslash escape rather than
    // `serde_json::to_string`, because this is standing in for the literal
    // bytes a pre-Task-8 `notes.json` would have carried, on either OS: a
    // Windows path with `\` in it must still land as valid JSON.
    let escaped = referenced.to_string_lossy().replace('\\', "\\\\");
    let json = LEGACY_NOTES_WITH_ATTACHMENT_JSON.replace("__PATH__", &escaped);
    std::fs::write(path, json).unwrap();
}

#[test]
fn migrating_a_legacy_attachment_whose_file_exists_copies_it_in_and_owns_it() {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("migrate_attachment_source.png");
    std::fs::write(&source, b"legacy attachment bytes, present on disk").unwrap();

    let path = dir.join("migrate_attachment_present.json");
    let machine_path = dir.join("migrate_attachment_present.machine.json");
    let attachments_dir = dir.join("migrate_attachment_present_attachments");
    let _ = std::fs::remove_file(&machine_path);
    let _ = std::fs::remove_dir_all(&attachments_dir);
    write_legacy_attachment_json(&path, &source);

    let store = NoteStore::load_from(path, machine_path, attachments_dir.clone());

    let note = &store.notes[0];
    assert_eq!(note.attachments.len(), 1, "migration must not drop the attachment");
    match note.attachments[0].location() {
        Some(Location::Owned { hash, ext }) => {
            assert_eq!(ext, "png");
            let copy = attachments_dir.join(format!("{hash}.{ext}"));
            assert!(copy.exists(), "migration must copy the bytes in, rather than only relabeling the record");
            assert_eq!(std::fs::read(&copy).unwrap(), b"legacy attachment bytes, present on disk");
        }
        other => panic!("expected the attachment to be adopted on load, got {other:?}"),
    }
    assert!(
        store.is_dirty(),
        "the rewritten location must reach notes.json on the next flush, or every \
         restart re-copies the same file and re-derives the same hash for nothing"
    );
    assert!(source.exists(), "migration copies the file, it does not move or delete the original");
}

#[test]
fn migrating_a_legacy_attachment_whose_file_is_missing_keeps_it_as_a_broken_reference() {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("migrate_attachment_gone.png");
    let _ = std::fs::remove_file(&missing);

    let path = dir.join("migrate_attachment_missing.json");
    let machine_path = dir.join("migrate_attachment_missing.machine.json");
    let attachments_dir = dir.join("migrate_attachment_missing_attachments");
    let _ = std::fs::remove_file(&machine_path);
    let _ = std::fs::remove_dir_all(&attachments_dir);
    write_legacy_attachment_json(&path, &missing);

    let store = NoteStore::load_from(path, machine_path, attachments_dir);

    let note = &store.notes[0];
    assert_eq!(
        note.attachments.len(), 1,
        "a broken reference is recoverable through Locate…; dropping it is not"
    );
    assert_eq!(
        note.attachments[0].location(),
        Some(&Location::External { path: missing }),
        "with nothing to copy, the attachment keeps its original, still-broken path"
    );
    assert!(
        store.is_dirty(),
        "the JSON shape itself was rewritten even though nothing was adopted; without \
         this the old path-only shape would never leave disk, and task_eval's own \
         strict parse of notes.json would break on it"
    );
}
