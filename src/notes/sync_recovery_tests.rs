//! Sync robustness tests: unreadable documents, salvage, delete propagation,
//! fresh-install handshakes, the genesis document, and save-failure retries.
//! Content sync lives in `sync_tests.rs`, document mechanics in
//! `sync_doc_tests.rs`.
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

/// defeats it, which the guard below says out loud rather than passing.
#[cfg(unix)]
#[test]
fn an_unreadable_document_is_never_written_over() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("unreadable");
    let doc = dir.join("notes.automerge");

    // Build a real corpus first, then take away the ability to read it.
    let mut original = Machine::open(&dir);
    original.notes.create("the corpus".into(), NoteColor::Purple, NoteOrigin::Dictated);
    original.flush();
    let corpus_id = original.notes.notes[0].id.clone();
    let intact = std::fs::read(&doc).unwrap();
    let mirror = std::fs::read_to_string(dir.join("notes.json")).unwrap();
    let tasks_mirror = std::fs::read_to_string(dir.join("tasks.json")).ok();
    drop(original);

    std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        std::fs::read(&doc).is_err(),
        "this test needs a document it genuinely cannot read; running as root defeats it"
    );

    let mut blocked = Machine::open(&dir);
    assert!(blocked.notes.load_error.is_some(), "the reason has to reach the status log");
    assert!(blocked.notes.notes.is_empty(), "nothing could be read, so nothing is on the board");

    // The user carries on and writes a note. Every tick from here would have
    // saved an empty-plus-one document over the original.
    blocked.notes.create("written while blind".into(), NoteColor::Teal, NoteOrigin::Dictated);
    blocked.flush();
    blocked.flush();

    std::fs::set_permissions(&doc, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        std::fs::read(&doc).unwrap(),
        intact,
        "the only copy of the corpus must come through untouched"
    );
    assert!(
        !dir.join("notes.automerge.corrupt").exists(),
        "nothing was corrupt, so nothing should have been renamed aside"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.json")).unwrap(),
        mirror,
        "the mirror is the second copy of the same corpus, and rewriting it from a store \
         that came up empty would destroy that one too"
    );
    assert_eq!(std::fs::read_to_string(dir.join("tasks.json")).ok(), tasks_mirror);
    assert!(
        std::fs::read_to_string(dir.join("machine.json")).unwrap().contains(&corpus_id),
        "every note looked gone, but only because nothing could be read. The GC must not \
         wipe machine.json on the strength of a file we never opened"
    );
}

#[test]
fn an_incoming_document_that_will_not_parse_is_moved_aside_before_the_save() {
    let dir = temp_dir("bad_incoming");
    let mut m = Machine::open(&dir);
    m.notes.create("ours".into(), NoteColor::Purple, NoteOrigin::Dictated);
    m.flush();

    // A delivery that is not a document. The flush that follows writes our own
    // to the same path, so leaving it there would destroy it.
    std::fs::write(m.document(), b"half a file, or none of one").unwrap();
    m.notes.create("also ours".into(), NoteColor::Rose, NoteOrigin::Dictated);
    m.flush();

    let preserved: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("notes.automerge.unreadable-"))
        .collect();
    assert_eq!(preserved.len(), 1, "the delivered bytes must be kept, found {preserved:?}");
    assert_eq!(
        std::fs::read(dir.join(&preserved[0])).unwrap(),
        b"half a file, or none of one",
        "kept verbatim, so a half-written file can be looked at rather than guessed about"
    );

    let reopened = Machine::open(&dir).notes;
    assert_eq!(note_bodies(&reopened).len(), 2, "our own notes still made it to disk");
    assert!(
        !m.notes.doc_file_moved(),
        "moving the file aside is also what stops the tick asking for the same failed \
         merge twice a second"
    );
}

#[test]
fn a_note_deleted_on_one_machine_stays_deleted_after_the_merge() {
    let a_dir = temp_dir("delete_wins_a");
    let b_dir = temp_dir("delete_wins_b");

    let mut a = Machine::open(&a_dir);
    let keep = a.notes.create("keep me".into(), NoteColor::Purple, NoteOrigin::Dictated);
    let doomed = a.notes.create("delete me".into(), NoteColor::Rose, NoteOrigin::Dictated);
    a.flush();
    std::fs::copy(a.document(), b_dir.join("notes.automerge")).unwrap();
    let mut b = Machine::open(&b_dir);

    // Callisto deletes the note while the laptop is editing it.
    a.notes.delete(&doomed);
    a.flush();
    b.notes.set_body(&doomed, "edited on the laptop".into());
    b.flush();

    carry_document(&b, &a);
    a.flush();

    assert!(
        a.notes.get(&doomed).is_none(),
        "an explicit delete outranks a concurrent edit. This falls out of the map-keyed \
         layout rather than being enforced anywhere, so it is pinned here: an automerge \
         upgrade that resurrected the note would otherwise pass every other test"
    );
    assert!(a.notes.get(&keep).is_some(), "the delete must take nothing else with it");
}

#[test]
fn a_fresh_install_that_has_already_saved_still_sees_an_incoming_corpus() {
    let a_dir = temp_dir("genesis_fresh_a");
    let b_dir = temp_dir("genesis_fresh_b");

    let mut a = Machine::open(&a_dir);
    a.notes.create("the whole corpus".into(), NoteColor::Purple, NoteOrigin::Dictated);
    a.notes.create("and a second note".into(), NoteColor::Teal, NoteOrigin::Dictated);
    a.flush();

    // A fresh install with nothing to seed. It writes one note of its own and
    // therefore a document of its own, which is all it takes: before the
    // genesis change existed, that document created its own root maps, and
    // the merge below then dropped whichever side's maps lost the coin flip.
    let mut b = Machine::open(&b_dir);
    b.notes.create("made on the laptop first".into(), NoteColor::Amber, NoteOrigin::Dictated);
    b.flush();
    assert!(b.document().exists());

    // The corpus arrives from the other machine.
    std::fs::copy(a.document(), b.document()).unwrap();
    b.flush();

    let bodies = note_bodies(&b.notes);
    assert!(bodies.iter().any(|x| x == "the whole corpus"), "incoming corpus lost: {bodies:?}");
    assert!(bodies.iter().any(|x| x == "and a second note"), "incoming corpus lost: {bodies:?}");
    assert!(bodies.iter().any(|x| x == "made on the laptop first"), "own note lost: {bodies:?}");
}

#[test]
fn a_fresh_install_with_no_notes_asks_for_nothing() {
    let dir = temp_dir("genesis_idle");
    let mut m = Machine::open(&dir);

    assert!(m.notes.needs_flush(), "the seed path leaves the document owed a write");
    m.flush();
    assert!(
        !m.document().exists(),
        "there are no notes and the root maps come from genesis, so there is nothing to save"
    );
    assert!(
        !m.notes.needs_flush() && !m.tasks.needs_flush(),
        "an empty document is settled, not outstanding. Reporting it as owed would make \
         every tick take a write lock on both signals and re-render every open sticky"
    );
}

#[test]
fn the_genesis_document_still_has_the_object_ids_everything_depends_on() {
    use automerge::{ReadDoc, ROOT};

    let doc = super::sync_doc::new_document();
    let notes = doc.get(ROOT, super::sync_doc::NOTES_KEY).unwrap().expect("a notes map").1;
    let tasks = doc.get(ROOT, super::sync_doc::TASKS_KEY).unwrap().expect("a tasks map").1;

    // `<counter>@<actor>`, the actor being the sixteen zero bytes nothing ever
    // writes as. Two machines agreeing on these two strings is the whole
    // reason their corpora merge instead of one replacing the other.
    let genesis_actor = "0".repeat(32);
    assert_eq!(notes.to_string(), format!("1@{genesis_actor}"));
    assert_eq!(tasks.to_string(), format!("2@{genesis_actor}"));

    assert_eq!(
        super::sync_doc::build_genesis(),
        include_bytes!("genesis.automerge"),
        "the committed genesis bytes no longer match what this automerge version builds. \
         Rerun `cargo test notes::sync_tests::regenerate_the_genesis_document -- --ignored` \
         and check that the object ids above are unchanged, because a document already on \
         disk carries the old ones"
    );
}

#[test]
fn two_documents_seeded_independently_both_keep_their_notes_after_a_merge() {
    let a_dir = temp_dir("genesis_indep_a");
    let b_dir = temp_dir("genesis_indep_b");

    let mut a = Machine::open(&a_dir);
    let mut b = Machine::open(&b_dir);
    a.notes.create("only on callisto".into(), NoteColor::Purple, NoteOrigin::Dictated);
    b.notes.create("only on the laptop".into(), NoteColor::Rose, NoteOrigin::Dictated);
    a.flush();
    b.flush();

    carry_document(&b, &a);
    a.flush();

    let bodies = note_bodies(&a.notes);
    assert!(bodies.iter().any(|x| x == "only on callisto"), "callisto's own note: {bodies:?}");
    assert!(bodies.iter().any(|x| x == "only on the laptop"), "the laptop's note: {bodies:?}");
}

/// Rewrite `src/notes/genesis.automerge`. Ignored, because it is a code
/// generator rather than a check: run it by hand after an automerge upgrade
/// that `the_genesis_document_still_has_the_object_ids_everything_depends_on`
/// has failed on, then commit the new bytes.
///
/// ```text
/// cargo test notes::sync_tests::regenerate_the_genesis_document -- --ignored
/// ```
#[test]
#[ignore]
fn regenerate_the_genesis_document() {
    let bytes = super::sync_doc::build_genesis();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/notes/genesis.automerge");
    std::fs::write(&path, &bytes).unwrap();
    eprintln!("wrote {} bytes to {}", bytes.len(), path.display());
}

/// The portable half of `an_unreadable_document_is_never_written_over`.
///
/// A directory where the document should be makes `fs::read` fail on every
/// platform, so the latch is exercised on Windows too. It cannot check that
/// the document's bytes survive, because a directory would refuse the rename
/// anyway, so it checks the two files that would otherwise go: the mirror,
/// rewritten from a store that came up empty, and `machine.json`, wiped by a
/// GC whose valid set is empty for the same reason.
#[test]
fn an_unreadable_document_does_not_take_the_other_files_with_it() {
    let dir = temp_dir("unreadable_portable");

    let mut original = Machine::open(&dir);
    let id = original.notes.create("the corpus".into(), NoteColor::Purple, NoteOrigin::Dictated);
    original.flush();
    let mirror = std::fs::read_to_string(dir.join("notes.json")).unwrap();
    drop(original);

    std::fs::remove_file(dir.join("notes.automerge")).unwrap();
    std::fs::create_dir(dir.join("notes.automerge")).unwrap();

    let mut blocked = Machine::open(&dir);
    assert!(blocked.notes.document_read_only(), "a path that cannot be read latches the store");
    assert!(blocked.notes.load_error.is_some());
    blocked.notes.create("written while blind".into(), NoteColor::Teal, NoteOrigin::Dictated);
    blocked.flush();
    blocked.flush();

    assert_eq!(std::fs::read_to_string(dir.join("notes.json")).unwrap(), mirror);
    assert!(std::fs::read_to_string(dir.join("machine.json")).unwrap().contains(&id));
}

/// Critical 1: a document save that fails on one tick must be retried on the
/// next, even when nothing new gets reconciled in between.
///
/// Portable, the same trick as the test above: pre-creating the exact path
/// `SyncDoc::save` would write its temp file to, as a directory, makes that
/// one write fail on every platform without touching permissions.
/// `notes.json` is a different filename and lands fine, which is exactly the
/// trap this reproduces: the mirror looks healthy, and it is never read back
/// once the document exists, so a document save that quietly stops being
/// retried loses the edit for good.
#[test]
fn a_document_save_that_fails_is_retried_on_the_next_clean_tick() {
    let dir = temp_dir("retry_after_failed_save");
    let blocker = dir.join(format!("notes.automerge.tmp.{}", std::process::id()));

    let mut machine = Machine::open(&dir);
    let id = machine.notes.create(
        "written on the tick whose save failed".into(),
        NoteColor::Purple,
        NoteOrigin::Dictated,
    );

    std::fs::create_dir(&blocker).unwrap();
    machine.flush();
    assert!(!machine.document().exists(), "the save had nowhere to write its temp file");
    assert!(
        std::fs::read_to_string(dir.join("notes.json")).unwrap().contains(&id),
        "the mirror has to have saved fine, which is what makes this a trap: nothing looks wrong"
    );

    // No new edit happens between the two ticks. The failed tick's own
    // reconcile already landed the note in the in-memory document, so a
    // `run_document_pass` that only compares heads before and after its own
    // reconcile would see a no-op here and never retry.
    std::fs::remove_dir(&blocker).unwrap();
    machine.flush();

    assert!(machine.document().exists(), "the retry has to actually reach disk");
    let reloaded = Machine::open(&dir);
    assert_eq!(
        reloaded.body(&id),
        "written on the tick whose save failed",
        "the edit from the failed tick has to reach the document itself; the mirror alone is not the corpus"
    );
}
