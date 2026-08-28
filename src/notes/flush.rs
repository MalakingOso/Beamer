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
use super::{doc_notes, doc_tasks, NoteStore};

/// What one pass over the document did.
struct DocPass {
    /// Something arrived from another machine and both vecs were rebuilt.
    merged: bool,
    /// The document file was rewritten.
    saved: bool,
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
    // failed document save leaves it set, which is the retry.
    if pass.saved {
        notes.doc_dirty = false;
        tasks.doc_dirty = false;
    }
    wrote
}

fn run_document_pass(notes: &mut NoteStore, tasks: &mut TaskStore) -> DocPass {
    let handle = notes.sync_doc();
    let mut doc = handle.lock();
    let before = doc.heads();

    if let Err(e) = doc_notes::reconcile(&mut doc, &notes.notes) {
        tracing::error!("Could not write notes into the sync document: {e}");
    }
    if let Err(e) = doc_tasks::reconcile(&mut doc, &tasks.tasks) {
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
            // Very likely a half-written file. Leave the recorded mtime alone
            // so the next tick tries again.
            Err(e) => tracing::warn!("Could not merge the incoming sync document: {e}"),
        }
    }

    if merged {
        notes.notes = doc_notes::hydrate(&doc);
        tasks.tasks = doc_tasks::hydrate(&doc);
    }

    let mut saved = merged || doc.heads() != before;
    if saved {
        if let Err(e) = doc.save() {
            tracing::error!("Failed to save the sync document: {e}");
            saved = false;
        }
    }
    DocPass { merged, saved }
}
