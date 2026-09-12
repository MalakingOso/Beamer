//! Tests for the automerge-backed corpus: merging, splicing, the mtime check,
//! the JSON mirror, and what happens when the document will not load.
//!
//! Every test builds its own temp directory. Nothing here reads or writes
//! anything under the real `~/.config/Beamer`, which holds the user's actual
//! notes.

use std::path::{Path, PathBuf};

use automerge::legacy::OpType;

use super::task::{Proposal, TaskStatus};
use super::task_store::TaskStore;
use super::{flush_stores, NoteColor, NoteOrigin, NoteStore, StageState};

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

/// The vocabulary rides the same document the notes do. `vocabulary.txt` sits
/// beside `notes.json`, so a machine's own config directory is what decides
/// which file this is — nothing here can reach the real one.
#[test]
fn a_vocabulary_edited_on_one_machine_reaches_the_other() {
    let a_dir = temp_dir("vocab_a");
    let b_dir = temp_dir("vocab_b");
    let mut a = Machine::open(&a_dir);
    let mut b = Machine::open(&b_dir);

    std::fs::write(a_dir.join("vocabulary.txt"), "Kubernetes\ntokio::spawn").unwrap();
    a.flush();

    carry_document(&a, &b);
    b.flush();

    assert_eq!(
        std::fs::read_to_string(b_dir.join("vocabulary.txt")).unwrap(),
        "Kubernetes\ntokio::spawn"
    );
}

/// The vocabulary must not drag the note corpus around with it: a machine
/// whose only change is a new term still has to leave the notes alone.
#[test]
fn carrying_a_vocabulary_does_not_disturb_the_notes() {
    let a_dir = temp_dir("vocab_notes_a");
    let b_dir = temp_dir("vocab_notes_b");
    let mut a = Machine::open(&a_dir);
    a.notes.create("a note that must survive".into(), NoteColor::Purple, NoteOrigin::Dictated);
    a.flush();

    std::fs::write(a_dir.join("vocabulary.txt"), "PostgreSQL").unwrap();
    a.flush();

    let mut b = Machine::open(&b_dir);
    carry_document(&a, &b);
    b.flush();

    assert_eq!(note_bodies(&b.notes), ["a note that must survive"]);
    assert_eq!(std::fs::read_to_string(b_dir.join("vocabulary.txt")).unwrap(), "PostgreSQL");
}

#[test]
fn two_documents_edited_offline_merge_into_one_that_holds_both_edits() {
    let a_dir = temp_dir("divergent_a");
    let b_dir = temp_dir("divergent_b");

    let mut a = Machine::open(&a_dir);
    a.notes.create("the note they both start with".into(), NoteColor::Purple, NoteOrigin::Dictated);
    a.flush();

    // The laptop receives the document and opens it, rather than seeding its
    // own from JSON. Since the genesis change both machines start from, an
    // independent seed merges per note instead of losing a whole root map,
    // but it still resolves each field by conflict rather than by splice.
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
fn decisions_made_on_two_machines_both_survive_the_merge() {
    let a_dir = temp_dir("tasks_a");
    let b_dir = temp_dir("tasks_b");

    let mut a = Machine::open(&a_dir);
    let note_id = a.notes.create("two things to do".into(), NoteColor::Teal, NoteOrigin::Dictated);
    let first = TaskStore::new_suggestion(
        &note_id,
        Proposal { text: "Call the vet".into(), evidence: "call the vet".into(), ..Default::default() },
        "test-machine",
    );
    let second = TaskStore::new_suggestion(
        &note_id,
        Proposal { text: "Post the form".into(), evidence: "post the form".into(), ..Default::default() },
        "test-machine",
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


/// The critical case: a document that is there and cannot be read.
///
/// Not a parse failure, so nothing gets quarantined, and the store used to
/// come up empty over intact bytes. The next edit then wrote a one-note
/// document over the user's whole corpus, with no `.corrupt` copy anywhere,
/// because from the store's point of view nothing had gone wrong.
///
/// Unix only: the failure needs a real `fs::read` error on a path that
/// `exists()`, and mode 000 is the portable way to get one. Running as root

#[test]
fn a_concurrent_failure_does_not_regress_a_synced_done() {
    let a_dir = temp_dir("stage_done_a");
    let b_dir = temp_dir("stage_done_b");

    let mut a = Machine::open(&a_dir);
    let note_id = a.notes.create("call the vet".into(), NoteColor::Teal, NoteOrigin::Dictated);
    a.notes.mark_analyzed(&note_id);
    a.flush();

    // B receives the done note, then its own retry fails against it.
    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();
    let mut b = Machine::open(&b_dir);
    assert_eq!(
        b.notes.get(&note_id).unwrap().extract_state,
        StageState::Done,
        "the laptop sees the completed pass"
    );
    b.notes.mark_extract_failed(&note_id);
    b.flush();

    // The shared record still says done: B's failure was real (its own store
    // still says so), but success is monotonic — a transient failure must not
    // regress it with no evidence either way.
    assert_eq!(b.notes.get(&note_id).unwrap().extract_state, StageState::Failed);
    carry_document(&b, &a);
    a.flush();
    assert_eq!(
        a.notes.get(&note_id).unwrap().extract_state,
        StageState::Done,
        "merging B's failure must not undo A's completed pass"
    );
}

#[test]
fn a_deliberate_skip_propagates_while_done_stays_put() {
    // Skipped is what the pipeline records for a never-run stage when the
    // feature is off — it propagates like any other state. What it can never
    // do is overwrite Done (see `skipped_never_overwrites_done`): success is
    // monotonic in the shared record, so turning the feature off after a pass
    // ran leaves that pass's record alone.
    let a_dir = temp_dir("stage_skip_a");
    let b_dir = temp_dir("stage_skip_b");

    let mut a = Machine::open(&a_dir);
    let note_id = a.notes.create("call the vet".into(), NoteColor::Teal, NoteOrigin::Dictated);
    a.flush();

    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();
    let mut b = Machine::open(&b_dir);
    // What the pipeline does on a Pending note while the pass is disabled.
    b.notes.mark_extract_skipped(&note_id);
    assert_eq!(b.notes.get(&note_id).unwrap().extract_state, StageState::Skipped);
    b.flush();

    carry_document(&b, &a);
    a.flush();
    assert_eq!(
        a.notes.get(&note_id).unwrap().extract_state,
        StageState::Skipped,
        "a pass that never ran stays visibly off after the merge"
    );

    // And the reverse: a completed pass is immune to a later skip.
    a.notes.mark_analyzed(&note_id);
    a.notes.mark_extract_skipped(&note_id);
    assert_eq!(
        a.notes.get(&note_id).unwrap().extract_state,
        StageState::Done,
        "disabling the feature must not rewrite a completed pass"
    );
}
