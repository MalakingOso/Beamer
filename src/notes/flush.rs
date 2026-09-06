//! The one place the automerge document is written. Ordering is the point:
//! 1. reconcile notes, reconcile tasks 2. merge the incoming file, if moved
//! 3. hydrate both stores back, only if the merge brought something new
//! 4. save the document, then the JSON mirrors. Merging before reconciling would
//! read as a deliberate revert of the other machine's edits.

use super::task_store::TaskStore;
use super::{doc_notes, doc_tasks, doc_vocab, NoteStore};

/// What one pass over the document did.
struct DocPass {
    /// Something arrived from another machine and both vecs were rebuilt.
    merged: bool,
    /// The document file was rewritten.
    saved: bool,
    /// The document owes nothing further. Anything else and the tick would take
    /// a write lock on both signals forever.
    settled: bool,
}

/// Reconcile, merge, save. Returns whether anything was written. Called from the
/// 500 ms tick and the tray's Quit handler (which must write before `process::exit`).
pub fn flush_stores(notes: &mut NoteStore, tasks: &mut TaskStore) -> bool {
    prune_orphan_rows(notes, tasks);
    let pass = run_document_pass(notes, tasks);

    // A merge rewrote both vecs, so both mirrors are stale regardless of dirty flags.
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

    // After the mirrors: `flush_if_dirty` sets `doc_dirty` itself, which would leave
    // the flag standing on state the document already holds. Failed saves retry via
    // `SyncDoc::save_failed`, consulted in `run_document_pass`.
    if pass.settled {
        notes.doc_dirty = false;
        tasks.doc_dirty = false;
    }
    wrote
}

/// Drop task rows whose note is gone. Deleting a note mutates both stores in
/// memory before one `flush_stores` call, but a crash between the two mirror
/// writes still strands rows for a deleted note; left alone they linger on no
/// page and poison the eval corpus with labels for text that no longer
/// exists. Only when the notes side is trustworthy — never over a failed
/// load, where "gone" means "unreadable" — and rows for unreadable (kept, not
/// deleted) notes are spared with it.
fn prune_orphan_rows(notes: &NoteStore, tasks: &mut TaskStore) {
    if notes.load_error.is_some() || notes.document_read_only() {
        return;
    }
    let before = tasks.tasks.len();
    tasks.tasks.retain(|t| {
        notes.get(&t.note_id).is_some() || notes.unreadable_notes.iter().any(|id| id == &t.note_id)
    });
    if tasks.tasks.len() != before {
        tasks.dirty = true;
    }
}

fn run_document_pass(notes: &mut NoteStore, tasks: &mut TaskStore) -> DocPass {
    let handle = notes.sync_doc();
    let mut doc = handle.lock();

    if doc.is_read_only() {
        // The disk bytes are the only copy and we could not read them: reconciling is
        // work for nothing, and the mirrors are held back by each `flush_if_dirty`.
        return DocPass { merged: false, saved: false, settled: true };
    }

    // A save that failed on an earlier tick already moved the heads, so a later tick
    // with no new edit reconciles to a no-op and the heads comparison below sees
    // nothing; `has_save_failed` (folded into `changed`) is what still catches that miss.
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
                // Marked seen either way, or an identical rewrite keeps `file_moved` true forever.
                doc.mark_seen();
            }
            Err(e) => {
                // Quarantine first: the save below writes this same path and would destroy the delivery.
                tracing::warn!("Could not merge the incoming sync document: {e}");
                doc.quarantine_incoming();
            }
        }
    }

    // After the merge, deliberately: an earlier compare would read a stale vocabulary
    // and mistake an incoming edit for no edit. A push here moves the heads like any edit.
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

    // `has_pending_save` catches what the heads comparison cannot: the live-sync
    // coroutine mutates this same document between ticks, possibly before `before` is
    // captured. Peeked, not taken, so a failed save below still asks again next tick.
    // `has_save_failed` is the same for our own reconcile landing on a failed-save tick.
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
