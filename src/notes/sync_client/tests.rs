//! Tests for [`super`]: the sync protocol driven in-process (no socket), and
//! the reconcile-before-hydrate regression. Own file for the 500-line limit.

use super::*;

#[test]
fn empty_url_never_starts() {
    assert!(!should_start(""));
    assert!(!should_start("   "));
}

#[test]
fn a_configured_url_starts() {
    assert!(should_start("wss://callisto.taila63f23.ts.net/sync"));
}

/// The protocol without a socket: two documents converge through
/// `sync::State` and `Message::encode`/`decode` alone. That is what
/// `run_connection`/`socket_task` carry; a real connection would only test tokio.
#[test]
fn two_documents_converge_over_encoded_messages() {
    use automerge::transaction::Transactable;
    use automerge::{AutoCommit, ReadDoc, ROOT};

    let mut a = AutoCommit::new();
    a.put(ROOT, "from_a", "hello").unwrap();
    a.commit();

    let mut b = AutoCommit::new();
    b.put(ROOT, "from_b", "world").unwrap();
    b.commit();

    let mut a_state = SyncState::new();
    let mut b_state = SyncState::new();

    // Drive both directions until neither has anything left to send.
    loop {
        let a_to_b = a.sync().generate_sync_message(&mut a_state);
        if let Some(msg) = a_to_b.clone() {
            let wire = msg.encode();
            let decoded = SyncMessage::decode(&wire).unwrap();
            b.sync().receive_sync_message(&mut b_state, decoded).unwrap();
        }
        let b_to_a = b.sync().generate_sync_message(&mut b_state);
        if let Some(msg) = b_to_a.clone() {
            let wire = msg.encode();
            let decoded = SyncMessage::decode(&wire).unwrap();
            a.sync().receive_sync_message(&mut a_state, decoded).unwrap();
        }
        if a_to_b.is_none() && b_to_a.is_none() {
            break;
        }
    }

    assert_eq!(a.get(ROOT, "from_b").unwrap().unwrap().0.to_str(), Some("world"));
    assert_eq!(b.get(ROOT, "from_a").unwrap().unwrap().0.to_str(), Some("hello"));
    assert_eq!(a.get_heads(), b.get_heads());
}

/// A connection drops mid-exchange and both `State`s are thrown away, as on a
/// real reconnect (nothing persists them). A fresh `State` still converges;
/// it just costs a fuller first message.
#[test]
fn a_dropped_connection_reconverges_with_a_fresh_state() {
    use automerge::transaction::Transactable;
    use automerge::{AutoCommit, ReadDoc, ROOT};

    let mut a = AutoCommit::new();
    a.put(ROOT, "note", "first draft").unwrap();
    a.commit();

    let mut b = AutoCommit::new();

    let mut a_state = SyncState::new();
    let mut b_state = SyncState::new();

    // One exchange, then the connection drops before convergence: only
    // a's first message ever reaches b.
    let first = a.sync().generate_sync_message(&mut a_state).expect("a has something to send");
    b.sync()
        .receive_sync_message(&mut b_state, SyncMessage::decode(&first.encode()).unwrap())
        .unwrap();
    assert_ne!(a.get_heads(), b.get_heads(), "the drop must land before convergence, or this proves nothing");

    // Reconnect: both sides start over with a fresh sync state, as
    // `run_connection` does on every call.
    let mut a_state = SyncState::new();
    let mut b_state = SyncState::new();
    loop {
        let a_to_b = a.sync().generate_sync_message(&mut a_state);
        if let Some(msg) = a_to_b.clone() {
            b.sync()
                .receive_sync_message(&mut b_state, SyncMessage::decode(&msg.encode()).unwrap())
                .unwrap();
        }
        let b_to_a = b.sync().generate_sync_message(&mut b_state);
        if let Some(msg) = b_to_a.clone() {
            a.sync()
                .receive_sync_message(&mut a_state, SyncMessage::decode(&msg.encode()).unwrap())
                .unwrap();
        }
        if a_to_b.is_none() && b_to_a.is_none() {
            break;
        }
    }

    assert_eq!(a.get_heads(), b.get_heads(), "a fresh state must still re-converge after a drop");
    assert_eq!(b.get(ROOT, "note").unwrap().unwrap().0.to_str(), Some("first draft"));
}

/// `encode` then `decode` round-trips a message: what the wire actually carries.
#[test]
fn message_framing_round_trips() {
    use automerge::transaction::Transactable;
    use automerge::{AutoCommit, ROOT};

    let mut doc = AutoCommit::new();
    doc.put(ROOT, "key", "value").unwrap();
    doc.commit();

    let mut state = SyncState::new();
    let msg = doc.sync().generate_sync_message(&mut state).expect("a fresh document has something to send");

    let wire = msg.clone().encode();
    let decoded = SyncMessage::decode(&wire).unwrap();

    assert_eq!(decoded, msg);
}

/// Pins reconcile-before-hydrate: an edit still only in the signal (the 500 ms
/// tick hasn't run) must survive an incoming change. Hydrating straight after
/// the merge drops it silently and for good (the store then matches the
/// document, so no later diff recovers it): characters vanish mid-word
/// whenever a sync message lands while the user types.
///
/// Drives `reconcile_receive_and_hydrate` directly, so no Dioxus runtime is needed.
#[test]
fn an_edit_still_only_in_the_signal_survives_an_incoming_change() {
    use super::super::{NoteColor, NoteOrigin};

    let base =
        std::env::temp_dir().join(format!("beamer_sync_client_test_{}", std::process::id()));
    let a_dir = base.join("reconcile_before_hydrate_a");
    let b_dir = base.join("reconcile_before_hydrate_b");
    for d in [&a_dir, &b_dir] {
        let _ = std::fs::remove_dir_all(d);
        std::fs::create_dir_all(d).unwrap();
    }

    // Machine A: one note, already reconciled and flushed, so the store
    // and the document agree before the test does anything interesting.
    let mut a_notes = NoteStore::load_from(
        a_dir.join("notes.json"),
        a_dir.join("machine.json"),
        a_dir.join("notes.automerge"),
    );
    let mut a_tasks = TaskStore::load_beside_at(&a_notes, a_dir.join("tasks.json"));
    let id = a_notes.create("first draft".into(), NoteColor::Purple, NoteOrigin::Dictated);
    crate::notes::flush_stores(&mut a_notes, &mut a_tasks);

    // Machine B: received A's document (a file copy stands in for a first
    // sync), made an edit of its own, and flushed.
    std::fs::copy(a_dir.join("notes.automerge"), b_dir.join("notes.automerge")).unwrap();
    let mut b_notes = NoteStore::load_from(
        b_dir.join("notes.json"),
        b_dir.join("machine.json"),
        b_dir.join("notes.automerge"),
    );
    let mut b_tasks = TaskStore::load_beside_at(&b_notes, b_dir.join("tasks.json"));
    b_notes.set_body(&id, "first draft, seen on the other machine".into());
    crate::notes::flush_stores(&mut b_notes, &mut b_tasks);

    // Back on A: an edit lands in the signal, but the 500 ms tick has not
    // run since, so it is not yet in A's document. This is the gap under test.
    a_notes.set_body(&id, "URGENT: first draft".into());
    assert!(a_notes.dirty, "the edit must be pending in the store, not yet reconciled");

    // Drive a real sync exchange, applying every message B sends through the
    // function under test on A's side. A fresh exchange takes two round trips
    // (a summary first, then the changes), so loop to convergence.
    let a_handle = a_notes.sync_doc();
    let b_handle = b_notes.sync_doc();
    let mut a_state = SyncState::new();
    let mut b_state = SyncState::new();

    loop {
        let b_to_a = {
            let mut guard = b_handle.lock();
            let m = guard.doc_mut().sync().generate_sync_message(&mut b_state);
            m
        };
        if let Some(msg) = b_to_a.clone() {
            reconcile_receive_and_hydrate(&a_handle, &a_notes, &a_tasks, &mut a_state, msg);
        }
        let a_to_b = {
            let mut guard = a_handle.lock();
            let m = guard.doc_mut().sync().generate_sync_message(&mut a_state);
            m
        };
        if let Some(msg) = a_to_b.clone() {
            let mut guard = b_handle.lock();
            guard.doc_mut().sync().receive_sync_message(&mut b_state, msg).unwrap();
        }
        if b_to_a.is_none() && a_to_b.is_none() {
            break;
        }
    }

    let hydrated_notes = doc_notes::hydrate(&a_handle.lock());
    let merged = hydrated_notes
        .notes
        .iter()
        .find(|n| n.id == id)
        .expect("the note must still be present after the merge");
    assert!(
        merged.body.starts_with("URGENT:"),
        "A's own in-flight edit, still only in the signal when the incoming message \
         arrived, must not be erased by the hydrate: got {:?}",
        merged.body
    );
    assert!(
        merged.body.contains("seen on the other machine"),
        "B's edit must also be present, or this merged nothing: {:?}",
        merged.body
    );
}
