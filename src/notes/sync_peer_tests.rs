//! Tests for a third-party peer on the shared document: Look, which reads
//! `ROOT.tasks`, writes only `ROOT.tasks[id].done` there, and keeps its own
//! data under `ROOT.look`. Beamer must neither revert that `done` with a
//! stale in-memory copy nor prune the root key it does not know.
//!
//! Every test builds its own temp directory; nothing touches `~/.config/Beamer`.

use std::path::{Path, PathBuf};

use automerge::sync::{State as SyncState, SyncDoc as _};
use automerge::transaction::Transactable;
use automerge::{ActorId, AutoCommit, ObjType, ReadDoc, ROOT};

use super::sync_client::reconcile_receive_and_hydrate;
use super::task::Proposal;
use super::task_store::TaskStore;
use super::{flush_stores, NoteColor, NoteOrigin, NoteStore};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("beamer_peer_test_{}", std::process::id()))
        .join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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

    fn done(&self, id: &str) -> bool {
        self.tasks.tasks.iter().find(|t| t.id == id).expect("task is present").done
    }

    /// One accepted, open task, flushed so store and document agree.
    fn with_accepted_task(&mut self) -> String {
        let note_id =
            self.notes.create("call the vet".into(), NoteColor::Teal, NoteOrigin::Dictated);
        let task = TaskStore::new_suggestion(
            &note_id,
            Proposal { text: "Call the vet".into(), evidence: "call the vet".into(), ..Default::default() },
            "test-machine",
        );
        let id = task.id.clone();
        self.tasks.replace_suggestions(&note_id, vec![task]);
        self.tasks.accept(&id);
        self.flush();
        id
    }
}

/// The peer's copy: the machine's document bytes under a fresh actor, as Look
/// holds it after its first sync.
fn peer_from(machine: &Machine) -> AutoCommit {
    let mut doc = AutoCommit::load(&std::fs::read(machine.document()).unwrap()).unwrap();
    doc.set_actor(ActorId::random());
    doc
}

/// What Look's "tick" does: `ROOT.tasks[id].done = true`, nothing else in `tasks`.
fn peer_ticks(peer: &mut AutoCommit, task_id: &str) {
    let (_, tasks) = peer.get(ROOT, "tasks").unwrap().expect("tasks root");
    let (_, row) = peer.get(&tasks, task_id).unwrap().expect("task row");
    peer.put(&row, "done", true).unwrap();
    peer.commit();
}

fn deliver_file(peer: &mut AutoCommit, to: &Machine) {
    std::fs::write(to.document(), peer.save()).unwrap();
}

/// Run a full sync exchange, applying the peer's messages through the live
/// client's own reconcile-then-receive core.
fn deliver_live(peer: &mut AutoCommit, to: &Machine) {
    let handle = to.notes.sync_doc();
    let (mut ours, mut theirs) = (SyncState::new(), SyncState::new());
    loop {
        let to_us = peer.sync().generate_sync_message(&mut theirs);
        if let Some(msg) = to_us.clone() {
            reconcile_receive_and_hydrate(&handle, &to.notes, &to.tasks, &mut ours, msg);
        }
        let to_peer = handle.lock().doc_mut().sync().generate_sync_message(&mut ours);
        if let Some(msg) = to_peer.clone() {
            peer.sync().receive_sync_message(&mut theirs, msg).unwrap();
        }
        if to_us.is_none() && to_peer.is_none() {
            break;
        }
    }
}

#[test]
fn a_peers_done_survives_a_reconcile_of_the_stale_open_row() {
    let mut a = Machine::open(&temp_dir("done_file"));
    let id = a.with_accepted_task();

    let mut look = peer_from(&a);
    peer_ticks(&mut look, &id);
    deliver_file(&mut look, &a);

    // The store still says `done=false`; the tick reconciles it before merging.
    assert!(!a.done(&id));
    a.flush();
    assert!(a.done(&id), "the merge hydrated the peer's tick into the store");

    // And a later tick with nothing new does not take it back.
    a.flush();
    let reread = Machine::open(&a.dir);
    assert!(reread.done(&id), "the tick is on disk, not just in memory");
}

#[test]
fn a_local_edit_to_another_field_of_the_same_task_keeps_the_peers_done() {
    let mut a = Machine::open(&temp_dir("done_and_due"));
    let id = a.with_accepted_task();

    let mut look = peer_from(&a);
    peer_ticks(&mut look, &id);
    deliver_file(&mut look, &a);

    // Beamer sets a due date before its next tick: the row is now dirty in
    // memory and still carries the stale `done=false`.
    a.tasks.set_due(&id, Some("2026-10-01".into()), true);
    a.flush();

    let task = a.tasks.tasks.iter().find(|t| t.id == id).unwrap();
    assert!(task.done, "only the changed field was written, so the peer's tick stands");
    assert_eq!(task.due.as_deref(), Some("2026-10-01"));
}

#[test]
fn a_peers_done_survives_the_live_sync_path() {
    let mut a = Machine::open(&temp_dir("done_live"));
    let id = a.with_accepted_task();

    let mut look = peer_from(&a);
    peer_ticks(&mut look, &id);
    deliver_live(&mut look, &a);

    let handle = a.notes.sync_doc();
    let guard = handle.lock();
    let (_, tasks) = guard.doc().get(ROOT, "tasks").unwrap().unwrap();
    let (_, row) = guard.doc().get(&tasks, id.as_str()).unwrap().unwrap();
    let done = guard.doc().get(&row, "done").unwrap().unwrap().0.to_bool();
    assert_eq!(done, Some(true), "the store's stale row was reconciled as a no-op");
}

#[test]
fn beamer_leaves_a_root_key_it_does_not_know_alone() {
    let mut a = Machine::open(&temp_dir("look_root"));
    a.with_accepted_task();

    let mut look = peer_from(&a);
    let root = look.put_object(ROOT, "look", ObjType::Map).unwrap();
    let todos = look.put_object(&root, "todos", ObjType::Map).unwrap();
    look.put(&todos, "t1", "a look to-do").unwrap();
    look.commit();
    deliver_file(&mut look, &a);

    a.flush();
    a.notes.create("a later note".into(), NoteColor::Purple, NoteOrigin::Dictated);
    a.flush();

    let saved = AutoCommit::load(&std::fs::read(a.document()).unwrap()).unwrap();
    let (_, root) = saved.get(ROOT, "look").unwrap().expect("ROOT.look survives");
    let (_, todos) = saved.get(&root, "todos").unwrap().unwrap();
    assert_eq!(
        saved.get(&todos, "t1").unwrap().unwrap().0.into_string().ok().as_deref(),
        Some("a look to-do")
    );
}
