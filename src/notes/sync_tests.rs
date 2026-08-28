//! Tests for the automerge-backed corpus: merging, splicing, the mtime check,
//! the JSON mirror, and what happens when the document will not load.
//!
//! Every test builds its own temp directory. Nothing here reads or writes
//! anything under the real `~/.config/Beamer`, which holds the user's actual
//! notes and, since attachments became content-addressed, the only copy of
//! some of their files.

use std::path::{Path, PathBuf};

use automerge::legacy::OpType;

use super::task::{Proposal, TaskStatus};
use super::task_store::TaskStore;
use super::{flush_stores, NoteColor, NoteOrigin, NoteStore};

/// A `notes.json` shaped exactly like the one this install has been writing,
/// with invented text. Fourteen notes, two archived, three attachments across
/// two of them, ascending creation times.
const FIXTURE: &str = include_str!("../../tests/fixtures/notes-14.json");

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

impl Machine {
    fn open(dir: &Path) -> Self {
        let notes = NoteStore::load_from(
            dir.join("notes.json"),
            dir.join("machine.json"),
            dir.join("attachments"),
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

    fn body(&self, id: &str) -> String {
        self.notes.get(id).expect("note is present").body.clone()
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
fn two_documents_edited_offline_merge_into_one_that_holds_both_edits() {
    let a_dir = temp_dir("divergent_a");
    let b_dir = temp_dir("divergent_b");

    let mut a = Machine::open(&a_dir);
    a.notes.create("the note they both start with".into(), NoteColor::Purple, NoteOrigin::Dictated);
    a.flush();

    // The laptop receives the document and opens it. It never seeds its own
    // from JSON: two independent seeds mint different automerge object ids
    // for the same notes and would conflict whole instead of merging.
    let b = Machine::open(&b_dir);
    drop(b);
    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();
    let mut b = Machine::open(&b_dir);
    assert_eq!(b.notes.notes.len(), 1, "the laptop opens the document it was handed");

    a.notes.create("written on callisto".into(), NoteColor::Teal, NoteOrigin::Dictated);
    a.flush();
    b.notes.create("written on the laptop".into(), NoteColor::Rose, NoteOrigin::Dictated);
    b.flush();

    carry_document(&b, &a);
    a.flush();

    let bodies = note_bodies(&a.notes);
    assert_eq!(bodies.len(), 3, "both machines' new notes survive the merge: {bodies:?}");
    assert!(bodies.iter().any(|b| b == "written on callisto"));
    assert!(bodies.iter().any(|b| b == "written on the laptop"));
}

#[test]
fn the_same_note_edited_on_both_machines_merges_character_by_character() {
    let a_dir = temp_dir("chars_a");
    let b_dir = temp_dir("chars_b");

    let mut a = Machine::open(&a_dir);
    let id = a.notes.create(
        "Call the vet about Biscuit".into(),
        NoteColor::Purple,
        NoteOrigin::Dictated,
    );
    a.flush();

    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();
    let mut b = Machine::open(&b_dir);

    // Callisto adds to the end, the laptop adds to the front. Disjoint spans,
    // so a character-level merge keeps both and a last-write-wins register
    // keeps exactly one.
    a.notes.set_body(&id, "Call the vet about Biscuit on Tuesday".into());
    a.flush();
    b.notes.set_body(&id, "Urgent: Call the vet about Biscuit".into());
    b.flush();

    carry_document(&b, &a);
    a.flush();

    let merged = a.body(&id);
    assert!(merged.contains("Urgent:"), "the laptop's edit is missing from {merged:?}");
    assert!(merged.contains("on Tuesday"), "callisto's edit is missing from {merged:?}");
    assert_eq!(
        merged.matches("Biscuit").count(),
        1,
        "the untouched middle must appear once, not once per machine: {merged:?}"
    );
}

#[test]
fn a_keystroke_becomes_a_splice_rather_than_a_whole_string_rewrite() {
    let dir = temp_dir("splice");
    let mut m = Machine::open(&dir);
    let body = "Ring the plumber about the kitchen tap before the weekend";
    let id = m.notes.create(body.into(), NoteColor::Purple, NoteOrigin::Dictated);
    m.flush();

    let handle = m.notes.sync_doc();
    let before = handle.lock().heads();

    // One keystroke at the end of a long body.
    m.notes.set_body(&id, format!("{body}!"));
    m.flush();

    let changes = {
        let mut doc = handle.lock();
        doc.doc_mut().get_changes(&before)
    };
    let ops: Vec<_> = changes.iter().flat_map(|c| c.decode().operations).collect();
    let inserts: Vec<_> = ops.iter().filter(|op| op.insert).collect();
    let deletes = ops.iter().filter(|op| matches!(op.action, OpType::Delete)).count();

    assert_eq!(
        inserts.len(),
        1,
        "one typed character must produce one insert. {} inserts means the body was \
         re-written whole, which is a last-write-wins register wearing a CRDT's clothes",
        inserts.len()
    );
    assert_eq!(
        deletes, 0,
        "nothing was removed, so a splice deletes nothing. A whole-string rewrite would \
         delete the {} characters that did not change",
        body.len()
    );
    assert!(
        matches!(&inserts[0].action, OpType::Put(v) if v.to_str() == Some("!")),
        "the insert must carry the typed character, got {:?}",
        inserts[0].action
    );
    assert!(
        ops.len() < body.len(),
        "a change of {} ops over a {}-character body is a rewrite, not a splice",
        ops.len(),
        body.len()
    );
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
    seeded.flush();

    // Delete the mirror, so the second load has nothing to fall back on and
    // everything below came out of the document.
    std::fs::remove_file(dir.join("notes.json")).unwrap();
    let reloaded = Machine::open(&dir).notes;
    reloaded.save().unwrap();

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("notes.json")).unwrap()).unwrap();
    let expected: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(written, expected, "the round trip through the document must lose nothing");
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

#[test]
fn decisions_made_on_two_machines_both_survive_the_merge() {
    let a_dir = temp_dir("tasks_a");
    let b_dir = temp_dir("tasks_b");

    let mut a = Machine::open(&a_dir);
    let note_id = a.notes.create("two things to do".into(), NoteColor::Teal, NoteOrigin::Dictated);
    let first = TaskStore::new_suggestion(
        &note_id,
        Proposal { text: "Call the vet".into(), evidence: "call the vet".into(), ..Default::default() },
    );
    let second = TaskStore::new_suggestion(
        &note_id,
        Proposal { text: "Post the form".into(), evidence: "post the form".into(), ..Default::default() },
    );
    let (first_id, second_id) = (first.id.clone(), second.id.clone());
    a.tasks.replace_suggestions(&note_id, vec![first, second]);
    a.flush();

    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();
    let mut b = Machine::open(&b_dir);
    assert_eq!(b.tasks.tasks.len(), 2, "the laptop sees both suggestions");

    a.tasks.accept(&first_id);
    a.flush();
    b.tasks.dismiss(&second_id);
    b.flush();

    carry_document(&b, &a);
    a.flush();

    let status = |id: &str| a.tasks.tasks.iter().find(|t| t.id == id).map(|t| t.status);
    assert_eq!(status(&first_id), Some(TaskStatus::Accepted));
    assert_eq!(
        status(&second_id),
        Some(TaskStatus::Dismissed),
        "a dismissal is a labelled negative and the scarce half of the corpus; losing it \
         to a merge would be the worst kind of quiet data loss"
    );
}
