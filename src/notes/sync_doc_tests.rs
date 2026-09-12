//! Document-mechanics sync tests: what happens when the other machine's
//! document arrives (merge-not-clobber), when it will not parse (corrupt,
//! quarantine, salvage), and the guarantees of the JSON mirror. Content sync
//! (notes, tasks, vocabulary, stage states) lives in `sync_tests.rs`.
//!
//! Every test builds its own temp directory. Nothing here reads or writes
//! anything under the real `~/.config/Beamer`.

use std::path::{Path, PathBuf};

use super::{flush_stores, NoteColor, NoteOrigin, NoteStore};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("beamer_sync_test_{}", std::process::id()))
        .join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One install: its two stores over one document, all under `dir`.
struct Machine {
    notes: NoteStore,
    tasks: TaskStore,
    dir: PathBuf,
}
use super::task_store::TaskStore;

/// A `notes.json` shaped like the one this install has been writing, with
/// invented text. Fourteen notes, two archived, ascending creation times,
/// and `pos`/`size`/`open` on every note, which the live file still carries
/// and the migration still has to lift into `machine.json`.
const FIXTURE: &str = include_str!("../../tests/fixtures/notes-14.json");

/// The fixture with the machine-local keys removed, which is what the mirror
/// looks like once they have been lifted out. `Note` stopped serializing them
/// two tasks ago, so this is the only honest thing to compare against.
fn fixture_without_window_keys() -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    for note in value["notes"].as_array_mut().unwrap() {
        let note = note.as_object_mut().unwrap();
        note.remove("pos");
        note.remove("size");
        note.remove("open");
    }
    value
}

impl Machine {
    fn open(dir: &Path) -> Self {
        let notes = NoteStore::load_from(
            dir.join("notes.json"),
            dir.join("machine.json"),
            dir.join("notes.automerge"),
        );
        let tasks = TaskStore::load_beside_at(&notes, dir.join("tasks.json"));
        Self { notes, tasks, dir: dir.to_path_buf() }
    }

    fn flush(&mut self) -> bool {
        flush_stores(&mut self.notes, &mut self.tasks)
    }

    fn document(&self) -> PathBuf {
        self.dir.join("notes.automerge")
    }
}

/// What a sync client does: drop one machine's document into the other's
/// directory. The mtime moves, which is the only signal Beamer gets.
fn carry_document(from: &Machine, to: &Machine) {
    std::fs::copy(from.document(), to.document()).unwrap();
}

fn note_bodies(store: &NoteStore) -> Vec<String> {
    store.notes.iter().map(|n| n.body.clone()).collect()
}

#[test]
fn a_document_written_by_another_machine_is_merged_rather_than_clobbered() {
    let a_dir = temp_dir("mtime_a");
    let b_dir = temp_dir("mtime_b");

    let mut a = Machine::open(&a_dir);
    a.notes.create("shared".into(), NoteColor::Purple, NoteOrigin::Dictated);
    a.flush();
    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();

    let mut b = Machine::open(&b_dir);
    b.notes.create("only the laptop has this".into(), NoteColor::Amber, NoteOrigin::Dictated);
    b.flush();
    carry_document(&b, &a);

    // Callisto has an edit of its own in hand, so its flush has something to
    // write. Without the mtime check that write would replace the file the
    // laptop just delivered.
    a.notes.create("only callisto has this".into(), NoteColor::Slate, NoteOrigin::Dictated);
    assert!(a.notes.doc_file_moved(), "the delivered file must be noticed");
    a.flush();

    let on_disk = Machine::open(&a_dir);
    let bodies = note_bodies(&on_disk.notes);
    assert_eq!(bodies.len(), 3, "the file on disk lost something: {bodies:?}");
    assert!(bodies.iter().any(|b| b == "only the laptop has this"));
    assert!(bodies.iter().any(|b| b == "only callisto has this"));
    assert!(
        !a.notes.doc_file_moved(),
        "our own write must reset the watermark, or every tick would take a write lock \
         on both signals forever"
    );
}

#[test]
fn a_corrupt_document_surfaces_an_error_instead_of_an_empty_store() {
    let dir = temp_dir("corrupt");
    std::fs::write(dir.join("notes.automerge"), b"this is not an automerge document").unwrap();

    let store = Machine::open(&dir).notes;

    assert!(
        store.load_error.is_some(),
        "an unreadable corpus replaced by an empty one, with nothing said, is the failure \
         this path exists to stop"
    );
    let message = store.load_error.clone().unwrap();
    assert!(message.contains("notes.automerge"), "the message must name the file: {message}");
    assert!(
        dir.join("notes.automerge.corrupt").exists(),
        "the unreadable bytes must be preserved, not overwritten by the next flush"
    );
}

#[test]
fn an_existing_notes_json_round_trips_through_the_document_and_back_out_unchanged() {
    let dir = temp_dir("roundtrip");
    std::fs::write(dir.join("notes.json"), FIXTURE).unwrap();

    // Seed the document from the legacy file, exactly as an install upgrading
    // to this task does.
    let mut seeded = Machine::open(&dir);
    assert_eq!(seeded.notes.notes.len(), 14, "every note in the file must reach the document");

    // The window keys go to `machine.json`, not into the document. Anything
    // else and one machine's geometry and open set would sync to the other.
    let first = seeded.notes.notes[0].id.clone();
    let second = seeded.notes.notes[1].id.clone();
    assert_eq!(seeded.notes.size(&first), Some((268, 208)));
    assert!(seeded.notes.is_open(&first));
    assert_eq!(seeded.notes.pos(&second), Some((1170, 640)));
    assert!(!seeded.notes.is_open(&second));
    seeded.flush();

    // Delete the mirror, so the second load has nothing to fall back on and
    // everything below came out of the document.
    std::fs::remove_file(dir.join("notes.json")).unwrap();
    let reloaded = Machine::open(&dir).notes;
    reloaded.save().unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("notes.json")).unwrap()).unwrap();
    assert_eq!(
        written,
        fixture_without_window_keys(),
        "the round trip through the document must lose nothing but the machine-local keys"
    );
    assert!(
        !std::fs::read_to_string(dir.join("notes.json")).unwrap().contains("\"open\""),
        "the mirror must not carry window state onward to the other machine"
    );
    let machine = std::fs::read_to_string(dir.join("machine.json")).unwrap();
    assert!(machine.contains(&first), "the lifted window state must land in machine.json");
}

#[test]
fn the_json_mirror_is_an_export_and_is_never_read_back() {
    let dir = temp_dir("mirror_is_export");
    let mut m = Machine::open(&dir);
    m.notes.create("what the document says".into(), NoteColor::Purple, NoteOrigin::Dictated);
    m.flush();

    // Somebody edits the mirror by hand, or a stale copy arrives.
    std::fs::write(dir.join("notes.json"), r#"{"notes":[]}"#).unwrap();

    let reopened = Machine::open(&dir).notes;
    assert_eq!(
        note_bodies(&reopened),
        vec!["what the document says".to_string()],
        "once the document exists, notes.json is a derived export and cannot resurrect \
         or delete anything"
    );
}

#[test]
fn a_superseded_cleanup_still_leaves_the_users_own_text_in_the_document() {
    let dir = temp_dir("cleanup_cas");
    let mut m = Machine::open(&dir);
    let id = m.notes.create("call the vet".into(), NoteColor::Purple, NoteOrigin::Dictated);
    m.flush();

    // The body captured when the request went out, then the user types while
    // the model is thinking.
    let sent_with = "call the vet".to_string();
    m.notes.set_body(&id, "call the vet about Biscuit".into());

    let outcome = m.notes.apply_cleanup(&id, &sent_with, "Call the vet.");
    m.flush();

    assert_eq!(outcome, super::lifecycle::StageOutcome::Superseded);
    let reloaded = Machine::open(&dir).notes;
    assert_eq!(
        reloaded.get(&id).unwrap().body,
        "call the vet about Biscuit",
        "character-merging a model's wholesale rewrite against a mid-flight edit would give \
         text that is neither. The compare-and-swap stays, and the user's edit wins"
    );
}
