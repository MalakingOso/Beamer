//! The one place the automerge document is written.
//!
//! Both stores reconcile into the document, then any copy that arrived from
//! another machine is merged in, then the result is written back and mirrored
//! to JSON. Ordering is the whole point of putting this in one function:
//!
//! 1. reconcile notes, reconcile tasks
//! 2. merge the incoming document, if the file moved since our last write
//! 3. hydrate both stores back, but only if the merge brought something new
//! 4. save the document, then the JSON mirrors
//!
//! Merging before reconciling would make the reconcile diff look like a
//! deliberate revert of everything the other machine did, and a store that
//! skipped step 3 while another store's flush triggered a merge would revert
//! it on its own next tick. That is why there is no per-store document write.

use super::task_store::TaskStore;
use super::{doc_notes, doc_tasks, doc_vocab, NoteStore};

/// What one pass over the document did.
struct DocPass {
    /// Something arrived from another machine and both vecs were rebuilt.
    merged: bool,
    /// The document file was rewritten.
    saved: bool,
    /// The document owes nothing further. True when it was written, when
    /// there was nothing to write, and when it is read-only and never will
    /// be. Anything else and the tick would take a write lock on both signals
    /// forever.
    settled: bool,
}

/// Reconcile, merge, save. Returns whether anything was written.
///
/// Called from the 500 ms tick in `ui::app_setup::setup_notes_flush` and from
/// the tray's Quit handler, which has to write before `process::exit` skips
/// every destructor.
pub fn flush_stores(notes: &mut NoteStore, tasks: &mut TaskStore) -> bool {
    let pass = run_document_pass(notes, tasks);

    // A merge rewrote both vecs, so both mirrors are stale whatever their
    // dirty flags said before.
    if pass.merged {
        notes.dirty = true;
        tasks.dirty = true;
    }

    let mut wrote = pass.saved;
    if notes.flush_if_dirty() {
        wrote = true;
    }
    if tasks.flush_if_dirty() {
        wrote = true;
    }

    // After the mirrors, because `flush_if_dirty` sets `doc_dirty` itself and
    // would otherwise leave the flag standing on state the document already
    // holds, making every subsequent tick take a write lock for nothing. A
    // failed document save is a separate thing from these two flags: see
    // `SyncDoc::save_failed`, which `run_document_pass` consults on its own
    // and which is what actually makes a failed save get retried.
    if pass.settled {
        notes.doc_dirty = false;
        tasks.doc_dirty = false;
    }
    wrote
}

fn run_document_pass(notes: &mut NoteStore, tasks: &mut TaskStore) -> DocPass {
    let handle = notes.sync_doc();
    let mut doc = handle.lock();

    if doc.is_read_only() {
        // The bytes on disk are the only copy of the corpus and we could not
        // read them. Reconciling into a document nobody will ever write is
        // work for nothing, and reporting it as outstanding would make every
        // tick take a write lock on both signals.
        //
        // The mirrors are held back too, by each store's own `flush_if_dirty`:
        // this store came up empty, so rewriting `notes.json` from it would
        // destroy the second copy as surely as saving would destroy the first.
        // Nothing written this session survives it. The reason reaches the
        // status log at startup, and that is all the user gets told.
        return DocPass { merged: false, saved: false, settled: true };
    }

    // `doc.has_save_failed()` is read again below, folded into `changed`.
    // Captured in words here rather than inline there: a save that failed on
    // an earlier tick already moved the document's heads on that tick
    // (`reconcile` mutates the document whether or not the save after it
    // succeeds), so a tick with no new edit since then reconciles to a
    // no-op and the heads comparison a few lines down sees nothing on its
    // own. `save_failed` is the one thing still watching for that miss.
    let before = doc.heads();

    if let Err(e) = doc_notes::reconcile(&mut doc, &notes.notes, &notes.unreadable_notes) {
        tracing::error!("Could not write notes into the sync document: {e}");
    }
    if let Err(e) = doc_tasks::reconcile(&mut doc, &tasks.tasks, &tasks.unreadable_tasks) {
        tracing::error!("Could not write tasks into the sync document: {e}");
    }

    let mut merged = false;
    if doc.file_moved() {
        match doc.merge_incoming() {
            Ok(new_changes) => {
                merged = new_changes;
                // Marked seen either way. A sync client that rewrote the file
                // with content we already had would otherwise leave
                // `file_moved` true forever, and every tick would take a
                // write lock on both signals for nothing.
                doc.mark_seen();
            }
            Err(e) => {
                // The save below writes our own document to this same path,
                // so leaving the file where it is would destroy the delivery
                // rather than retry it. Move it aside first, keeping the
                // bytes; a client that really was mid-write delivers again.
                tracing::warn!("Could not merge the incoming sync document: {e}");
                doc.quarantine_incoming();
            }
        }
    }

    // After the merge, deliberately: the merge is what brings the other
    // machine's copy in, so comparing against `ROOT["vocabulary"]` any earlier
    // would read a stale value and mistake an incoming edit for no edit. A
    // push from here moves the heads, so the `changed` check below picks it up
    // the same way it picks up a note edit.
    if let Err(e) = doc_vocab::reconcile(&mut doc) {
        tracing::error!("Could not keep the vocabulary in step with the sync document: {e}");
    }

    if merged {
        let hydrated = doc_notes::hydrate(&doc);
        notes.notes = hydrated.notes;
        notes.unreadable_notes = hydrated.unreadable;
        let hydrated = doc_tasks::hydrate(&doc);
        tasks.tasks = hydrated.tasks;
        tasks.unreadable_tasks = hydrated.unreadable;
    }

    // Nothing changed is a settled document, not an outstanding one. A fresh
    // install with no notes reconciles to zero operations, because the root
    // maps arrive with the genesis change rather than being created here.
    //
    // `has_pending_save` catches what the heads comparison cannot: the
    // live-sync coroutine (`sync_client`) applies a peer's changes directly
    // to this same document between ticks, so by the time `before` is
    // captured above it can already include that mutation. Without this,
    // that change would sit correctly in the note/task signals and never
    // reach disk. Peeked rather than taken: if the save below fails, the
    // flag must survive to ask again next tick, since nothing else about
    // this pass would notice the miss a second time, since the mutation predates
    // `before` on every subsequent tick as much as it does on this one.
    //
    // `has_save_failed` is the same reasoning applied to our own reconcile
    // rather than the sync coroutine's: a local edit that already landed in
    // the document on a tick whose save then failed is otherwise invisible
    // to every comparison above, on every tick after the one that made it.
    let changed = merged || doc.heads() != before || doc.has_pending_save() || doc.has_save_failed();
    let mut saved = changed;
    if changed {
        match doc.save() {
            Ok(()) => doc.clear_pending_save(),
            Err(e) => {
                tracing::error!("Failed to save the sync document: {e}");
                saved = false;
            }
        }
    }
    DocPass { merged, saved, settled: saved || !changed }
}
