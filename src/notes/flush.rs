//! The app's only writer of the automerge document, plus the JSON mirrors
//! (`notes.json`, `tasks.json`). Run by the 500 ms tick and on quit.
//!
//! Ordering is the point: reconcile notes and tasks, merge the incoming file
//! if it moved, sync the vocabulary, hydrate both stores only if the merge
//! brought something new, then save the document and the mirrors. Merging
//! before reconciling would read as a deliberate revert of the other machine's edits.

use std::collections::HashSet;
use std::path::PathBuf;

use dioxus::prelude::*;

use super::sync_doc::{self, SyncHandle};
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

/// Reconcile, merge, save. Returns whether anything was written. Called from
/// the 500 ms tick, the tray's Quit handler (which must write before
/// `process::exit`), and the few paths that need an immediate write.
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

/// Drop task rows whose note is gone. A crash between the two mirror writes
/// can strand them, showing on no page and poisoning the eval corpus. Skipped
/// over a failed or read-only load (there "gone" means "unreadable"), and rows
/// for unreadable-but-kept notes are spared.
fn prune_orphan_rows(notes: &NoteStore, tasks: &mut TaskStore) {
    if notes.load_error.is_some() || notes.document_read_only() {
        return;
    }
    let valid_ids: HashSet<&str> = notes
        .notes
        .iter()
        .map(|n| n.id.as_str())
        .chain(notes.unreadable_notes.iter().map(String::as_str))
        .collect();
    let before = tasks.tasks.len();
    tasks.tasks.retain(|t| valid_ids.contains(t.note_id.as_str()));
    if tasks.tasks.len() != before {
        tasks.dirty = true;
    }
}

fn is_io_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>().is_some()
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

    let mut reconcile_ok = true;
    if let Err(e) = doc_notes::reconcile(&mut doc, &notes.notes, &notes.unreadable_notes) {
        tracing::error!("Could not write notes into the sync document: {e}");
        reconcile_ok = false;
    }
    if let Err(e) = doc_tasks::reconcile(&mut doc, &tasks.tasks, &tasks.unreadable_tasks) {
        tracing::error!("Could not write tasks into the sync document: {e}");
        reconcile_ok = false;
    }

    let mut merged = false;
    if doc.file_moved() {
        match doc.merge_incoming() {
            Ok(new_changes) => {
                merged = new_changes;
                // Marked seen either way, or an identical rewrite keeps `file_moved` true forever.
                doc.mark_seen();
            }
            Err(e) if is_io_error(&e) => {
                // Transient Windows lock/sharing violation: leave the file in place
                // so the next tick retries instead of quarantining an intact sync file.
                tracing::warn!("Could not read incoming sync document (will retry): {e}");
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

    // Never overwrite local in-memory edits with a hydrate when local reconcile failed.
    if merged && reconcile_ok {
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

/// Check off the UI thread whether `notes.automerge` moved on disk since our last write.
pub async fn doc_file_moved_async(doc: SyncHandle) -> bool {
    let (path, last_write) = {
        let g = doc.lock();
        if g.path().as_os_str().is_empty() {
            return false;
        }
        (g.path().to_path_buf(), g.last_write())
    };
    tokio::task::spawn_blocking(move || match (sync_doc::mtime(&path), last_write) {
        (Some(on_disk), Some(ours)) => on_disk != ours,
        (Some(_), None) => true,
        (None, _) => false,
    })
    .await
    .unwrap_or(false)
}

/// Non-blocking periodic flush for the 500ms UI tick: prepares in-memory
/// snapshots on the UI thread, then runs all file writes and Windows
/// `rename_with_retry` sleeps on `spawn_blocking` so the UI never freezes.
pub async fn flush_stores_async(
    mut notes: Signal<NoteStore>,
    mut tasks: Signal<TaskStore>,
    file_moved: bool,
) {
    // 1. If the document file moved on disk, read its bytes and mtime off the UI thread first.
    let doc_handle = notes.peek().sync_doc();
    let incoming_read: Option<(std::io::Result<Vec<u8>>, Option<std::time::SystemTime>)> = if file_moved {
        let path = doc_handle.lock().path().to_path_buf();
        if path.as_os_str().is_empty() {
            None
        } else {
            tokio::task::spawn_blocking(move || {
                let bytes = std::fs::read(&path);
                let mt = sync_doc::mtime(&path);
                (bytes, mt)
            })
            .await
            .ok()
        }
    } else {
        None
    };

    // 2. Reconcile in memory and snapshot serialized payloads (no disk writes on UI thread).
    let (doc_write, notes_write, machine_write, tasks_write, vocab_path, quarantine_incoming) = {
        let mut n = notes.write();
        let mut t = tasks.write();
        prune_orphan_rows(&n, &mut t);

        let mut doc = doc_handle.lock();
        let mut doc_write: Option<(PathBuf, Vec<u8>)> = None;
        let mut merged = false;
        let mut reconcile_ok = true;
        let mut quarantine_incoming = false;

        if !doc.is_read_only() {
            let before = doc.heads();
            if let Err(e) = doc_notes::reconcile(&mut doc, &n.notes, &n.unreadable_notes) {
                tracing::error!("Could not write notes into the sync document: {e}");
                reconcile_ok = false;
            }
            if let Err(e) = doc_tasks::reconcile(&mut doc, &t.tasks, &t.unreadable_tasks) {
                tracing::error!("Could not write tasks into the sync document: {e}");
                reconcile_ok = false;
            }

            if let Some((read_res, disk_mtime)) = incoming_read {
                match read_res {
                    Ok(bytes) => match doc.merge_bytes(&bytes) {
                        Ok(new_changes) => {
                            merged = new_changes;
                            doc.set_last_write(disk_mtime);
                        }
                        Err(e) => {
                            tracing::warn!("Could not merge the incoming sync document: {e}");
                            quarantine_incoming = true;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Could not read incoming sync document (will retry): {e}");
                    }
                }
            }

            if merged && reconcile_ok {
                let hydrated = doc_notes::hydrate(&doc);
                n.notes = hydrated.notes;
                n.unreadable_notes = hydrated.unreadable;
                let hydrated = doc_tasks::hydrate(&doc);
                t.tasks = hydrated.tasks;
                t.unreadable_tasks = hydrated.unreadable;
                n.dirty = true;
                t.dirty = true;
            }

            let changed =
                merged || doc.heads() != before || doc.has_pending_save() || doc.has_save_failed();
            if changed && !doc.path().as_os_str().is_empty() {
                doc_write = Some((doc.path().to_path_buf(), doc.doc_mut().save()));
                doc.clear_pending_save();
                doc.set_save_failed(false);
            }
        }
        let vocab_path = doc.vocab_path().map(PathBuf::from);
        drop(doc);

        let (notes_write, machine_write) = n.take_mirror_writes();
        let tasks_write = t.take_mirror_write();
        n.doc_dirty = false;
        t.doc_dirty = false;

        (doc_write, notes_write, machine_write, tasks_write, vocab_path, quarantine_incoming)
    };

    // 3. Perform all disk I/O + rename retries + quarantine + vocab sync on a blocking worker thread.
    let handle_for_io = doc_handle.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        if quarantine_incoming {
            handle_for_io.lock().quarantine_incoming();
        }
        let mut doc_err = false;
        let mut doc_mtime = None;
        if let Some((path, bytes)) = doc_write {
            match sync_doc::write_atomic_bytes(&path, "automerge", &bytes) {
                Ok(()) => doc_mtime = sync_doc::mtime(&path),
                Err(e) => {
                    tracing::error!("Failed to save the sync document: {e}");
                    doc_err = true;
                }
            }
        }
        let notes_err = if let Some((path, bytes)) = notes_write {
            sync_doc::write_atomic_bytes(&path, "json", &bytes)
                .map_err(|e| tracing::error!("Failed to save notes: {e}"))
                .is_err()
        } else {
            false
        };
        let machine_err = if let Some((path, bytes)) = machine_write {
            sync_doc::write_atomic_bytes(&path, "json", &bytes)
                .map_err(|e| tracing::error!("Failed to save machine state: {e}"))
                .is_err()
        } else {
            false
        };
        let tasks_err = if let Some((path, bytes)) = tasks_write {
            sync_doc::write_atomic_bytes(&path, "json", &bytes)
                .map_err(|e| tracing::error!("Failed to save tasks: {e}"))
                .is_err()
        } else {
            false
        };
        if vocab_path.is_some() {
            let mut doc = handle_for_io.lock();
            if !doc.is_read_only() {
                let before_vocab = doc.heads();
                if let Err(e) = doc_vocab::reconcile(&mut doc) {
                    tracing::error!("Could not keep the vocabulary in step with the sync document: {e}");
                }
                if doc.heads() != before_vocab {
                    doc.mark_pending_save();
                }
            }
        }
        (doc_err, doc_mtime, notes_err, machine_err, tasks_err)
    })
    .await;

    // 4. Record completion mtime and restore dirty flags on any failed write so next tick retries.
    if let Ok((doc_err, doc_mtime, notes_err, machine_err, tasks_err)) = outcome {
        if let Some(m) = doc_mtime {
            doc_handle.lock().set_last_write(Some(m));
        }
        if doc_err {
            doc_handle.lock().set_save_failed(true);
        }
        if notes_err || machine_err {
            let mut n = notes.write();
            if notes_err {
                n.dirty = true;
            }
            if machine_err {
                n.restore_machine_dirty();
            }
        }
        if tasks_err {
            tasks.write().dirty = true;
        }
    }
}
