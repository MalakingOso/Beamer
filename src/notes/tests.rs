//! Tests for `mod.rs`, split out under `#[path]` for the same reason
//! `task_store/tests.rs` and `pipeline/tests.rs` are: `mod.rs` was closing in
//! on the project's 500-line limit and this task added a machine-id scheme
//! and a migration path, each of which wants its own tests.

use super::*;

fn temp_store(tag: &str) -> NoteStore {
    let dir = std::env::temp_dir().join(format!("beamer_notes_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{tag}.json"));
    let machine_path = dir.join(format!("{tag}.machine.json"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&machine_path);
    NoteStore {
        notes: Vec::new(),
        path,
        dirty: false,
        machine: MachineStore::new(machine_path),
        doc: sync_doc::SyncHandle::default(),
        doc_dirty: false,
        load_error: None,
        unreadable_notes: Vec::new(),
    }
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
        "raw is the only record of what was actually said and must survive an edit"
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
    // An edit rewrote the body and dropped the filler word.
    store.set_body(&id, "Call the vet about Biscuit.".into());

    assert_eq!(store.search("biscuit").len(), 1, "matching must be case-insensitive");
    assert_eq!(store.search("VET").len(), 1);
    assert_eq!(
        store.search("um so").len(),
        1,
        "raw is searched too, an edit can remove the very words you remember saying"
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

#[test]
fn set_all_open_closes_every_active_note_and_leaves_archived_flags_alone() {
    let mut store = temp_store("hide_all");
    let a = store.create("a".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let b = store.create("b".into(), NoteColor::Teal, NoteOrigin::Dictated);
    let c = store.create("c".into(), NoteColor::Rose, NoteOrigin::Dictated);
    store.archive(&c);
    // `archive` always closes; force the flag open to prove the bulk path skips it.
    store.set_open(&c, true);

    assert!(store.any_active_open());

    store.set_all_open(false);

    assert!(!store.is_open(&a));
    assert!(!store.is_open(&b));
    assert!(!store.any_active_open());
    assert!(
        store.is_open(&c),
        "archived notes are outside hide-all's reach — and symmetrically, show-all \
         must never arm one to pop a window on restore"
    );
}

#[test]
fn set_all_open_reopens_every_active_note_but_never_an_archived_one() {
    let mut store = temp_store("show_all");
    let a = store.create("a".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let b = store.create("b".into(), NoteColor::Teal, NoteOrigin::Dictated);
    store.set_open(&a, false);
    store.set_open(&b, false);
    store.archive(&b);

    store.set_all_open(true);

    assert!(store.is_open(&a));
    assert!(
        !store.is_open(&b),
        "show-all must not reopen an archived note — and must not arm it either, \
         or restoring it later would pop a window unasked"
    );

    store.restore(&b);
    assert!(
        !store.is_open(&b),
        "restoring puts a note back on the board; it does not pop a window open"
    );
}

#[test]
fn set_all_open_on_an_empty_store_schedules_no_write() {
    let mut store = temp_store("bulk_empty");
    store.flush_if_dirty();

    store.set_all_open(false);
    store.set_all_open(true);

    assert!(!store.is_dirty(), "with no notes there is nothing to persist");
}

#[test]
fn set_all_open_never_moves_the_modified_timestamp() {
    let mut store = temp_store("bulk_modified");
    let id = store.create("hello".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let before = store.get(&id).unwrap().modified.clone();

    store.set_all_open(false);
    store.set_all_open(true);

    assert_eq!(
        store.get(&id).unwrap().modified,
        before,
        "bulk open/close is machine-local window state, like `set_open` — it must \
         never outrank a real edit under a last-write-wins merge"
    );
}
