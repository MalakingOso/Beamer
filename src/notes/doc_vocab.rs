//! Mapping between `vocabulary.txt` and the `vocabulary` scalar at the document root.
//! A last-write-wins scalar at `ROOT` (not a map): the list's order decides which
//! terms survive the `keyterms` cap, and in-place renames must not become
//! remove-then-add. `ROOT` is shared by every document, so no genesis entry is needed.

use anyhow::Result;
use automerge::ROOT;

use super::sync_doc::{get_str, put_str, SyncDoc};

/// Root key holding the vocabulary, as one newline-joined string.
pub const VOCAB_KEY: &str = "vocabulary";

/// Bring `vocabulary.txt` and the document's copy back into step. A no-op
/// without a vocabulary path. Runs after the merge, so the document side
/// already includes what the other machine sent.
pub fn reconcile(sync: &mut SyncDoc) -> Result<()> {
    let Some(path) = sync.vocab_path().map(std::path::Path::to_path_buf) else {
        return Ok(());
    };

    // `try_exists`, not `exists`: reading an unreadable file as "absent" would
    // push an empty list into the document — a deletion of every term.
    let file = match path.try_exists() {
        Ok(true) => Some(std::fs::read_to_string(&path)?),
        Ok(false) => None,
        Err(e) => {
            anyhow::bail!("Could not tell whether {} exists: {e}", path.display());
        }
    };
    let doc = get_str(sync.doc(), &ROOT, VOCAB_KEY);

    match decide(file.as_deref(), doc.as_deref(), sync.last_vocab()) {
        VocabAction::Idle => {}
        VocabAction::PushToDoc(content) => {
            put_str(sync.doc_mut(), &ROOT, VOCAB_KEY, &content)?;
            sync.mark_pending_save(); // or the push sits in memory until an unrelated edit flushes it
            sync.set_last_vocab(content);
        }
        VocabAction::WriteToFile(content) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, &content)?;
            sync.set_last_vocab(content);
        }
    }
    Ok(())
}

/// What a three-way comparison decided.
#[derive(Debug, PartialEq)]
pub enum VocabAction {
    Idle,
    /// The file moved: put its contents into the document.
    PushToDoc(String),
    /// The document moved: write its contents to the file.
    WriteToFile(String),
}

/// Decide what one pass owes, from the file, the document, and the baseline
/// both sides agreed on last pass (`None` on the first pass: no baseline yet).
/// The document wins whenever it moved, so a collision resolves identically
/// on both machines instead of ping-ponging.
pub fn decide(file: Option<&str>, doc: Option<&str>, last: Option<&str>) -> VocabAction {
    // A vanished key is not an instruction to blank the file.
    if doc != last {
        if let Some(doc) = doc {
            return VocabAction::WriteToFile(doc.to_string());
        }
    }
    if file != last {
        if let Some(file) = file {
            return VocabAction::PushToDoc(file.to_string());
        }
    }
    VocabAction::Idle
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::sync_doc::{get_str, put_str, SyncDoc};
    use automerge::ROOT;
    use std::path::PathBuf;

    /// A document plus vocab path under a PID-scoped temp dir, off the real config.
    fn temp_sync(tag: &str) -> (SyncDoc, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("beamer_docvocab_test_{}", std::process::id()))
            .join(tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let vocab_path = dir.join("vocabulary.txt");
        let (mut sync, _) = SyncDoc::open(dir.join("notes.automerge"));
        sync.set_vocab_path(vocab_path.clone());
        (sync, vocab_path)
    }

    fn doc_vocabulary(sync: &SyncDoc) -> Option<String> {
        get_str(sync.doc(), &ROOT, VOCAB_KEY)
    }

    #[test]
    fn a_local_list_reaches_the_document() {
        let (mut sync, path) = temp_sync("local_to_doc");
        std::fs::write(&path, "alpha\nbeta").unwrap();

        reconcile(&mut sync).unwrap();

        assert_eq!(doc_vocabulary(&sync).as_deref(), Some("alpha\nbeta"));
    }

    #[test]
    fn an_incoming_list_reaches_the_file() {
        let (mut sync, path) = temp_sync("doc_to_local");
        put_str(sync.doc_mut(), &ROOT, VOCAB_KEY, "gamma\ndelta").unwrap();

        reconcile(&mut sync).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "gamma\ndelta");
    }

    #[test]
    fn pushing_a_local_list_marks_the_document_for_saving() {
        let (mut sync, path) = temp_sync("marks_pending");
        std::fs::write(&path, "alpha").unwrap();

        reconcile(&mut sync).unwrap();

        assert!(sync.has_pending_save());
    }

    /// A settled second pass must claim no save, or every tick rewrites the document.
    #[test]
    fn a_settled_pass_writes_nothing() {
        let (mut sync, path) = temp_sync("settled");
        std::fs::write(&path, "alpha").unwrap();
        reconcile(&mut sync).unwrap();
        sync.clear_pending_save();

        reconcile(&mut sync).unwrap();

        assert!(!sync.has_pending_save());
    }

    #[test]
    fn a_document_with_no_vocabulary_path_is_left_alone() {
        let dir = std::env::temp_dir()
            .join(format!("beamer_docvocab_test_{}", std::process::id()))
            .join("no_path");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut sync, _) = SyncDoc::open(dir.join("notes.automerge"));

        reconcile(&mut sync).unwrap();

        assert_eq!(doc_vocabulary(&sync), None);
        assert!(!sync.has_pending_save());
    }

    #[test]
    fn nothing_changed_is_idle() {
        let action = decide(Some("alpha"), Some("alpha"), Some("alpha"));
        assert_eq!(action, VocabAction::Idle);
    }

    #[test]
    fn a_local_edit_is_pushed_into_the_document() {
        let action = decide(Some("alpha\nbeta"), Some("alpha"), Some("alpha"));
        assert_eq!(action, VocabAction::PushToDoc("alpha\nbeta".to_string()));
    }

    #[test]
    fn an_incoming_edit_is_written_to_the_file() {
        let action = decide(Some("alpha"), Some("alpha\nbeta"), Some("alpha"));
        assert_eq!(action, VocabAction::WriteToFile("alpha\nbeta".to_string()));
    }

    /// With both sides changed there is no merge; both machines must pick the same side.
    #[test]
    fn when_both_sides_changed_the_document_wins() {
        let action = decide(Some("alpha\nlocal"), Some("alpha\nremote"), Some("alpha"));
        assert_eq!(action, VocabAction::WriteToFile("alpha\nremote".to_string()));
    }

    #[test]
    fn with_no_baseline_a_populated_document_wins() {
        let action = decide(Some("local"), Some("remote"), None);
        assert_eq!(action, VocabAction::WriteToFile("remote".to_string()));
    }

    #[test]
    fn with_no_baseline_an_empty_document_is_seeded_from_the_file() {
        let action = decide(Some("alpha\nbeta"), None, None);
        assert_eq!(action, VocabAction::PushToDoc("alpha\nbeta".to_string()));
    }

    /// Neither side populated must not write an empty string as a "change".
    #[test]
    fn a_machine_with_neither_side_populated_stays_idle() {
        assert_eq!(decide(None, None, None), VocabAction::Idle);
    }

    /// An empty file with a populated baseline is a real deletion, not absence.
    #[test]
    fn clearing_every_term_locally_still_propagates() {
        let action = decide(Some(""), Some("alpha"), Some("alpha"));
        assert_eq!(action, VocabAction::PushToDoc(String::new()));
    }
}
