//! Mapping between `vocabulary.txt` and the `vocabulary` scalar at the
//! document root.
//!
//! Unlike `doc_notes` and `doc_tasks`, this is a **scalar at `ROOT`, not a
//! map**, and it is last-write-wins rather than merged. Two reasons, both
//! specific to this list: the vocabulary's order decides which terms survive
//! `keyterms`' 100/50-term cap, and an automerge map has no order; and
//! `Vocabulary::rename` edits a term in place on purpose, which under a
//! term-keyed map would become the remove-then-add its doc comment warns
//! against. A scalar keeps both properties for free, and `ROOT` is the one
//! object id every document already shares, so this needs no entry in
//! `genesis.automerge`. See
//! `docs/plans/2026-08-29-vocabulary-sync-design.md`.

use anyhow::Result;
use automerge::ROOT;

use super::sync_doc::{get_str, put_str, SyncDoc};

/// Root key holding the vocabulary, as one newline-joined string.
pub const VOCAB_KEY: &str = "vocabulary";

/// Bring `vocabulary.txt` and the document's copy back into step.
///
/// A no-op for a document with no vocabulary path, which is every document
/// `sync_server` opens. Called from `flush::run_document_pass` *after* the
/// merge, so the document side of the comparison already includes anything
/// the other machine sent.
pub fn reconcile(sync: &mut SyncDoc) -> Result<()> {
    let Some(path) = sync.vocab_path().map(std::path::Path::to_path_buf) else {
        return Ok(());
    };

    // `try_exists`, not `exists`, and an unreadable file is left alone rather
    // than read as absent — the same distinction `Vocabulary::load` draws,
    // and for a sharper reason here: "absent" would push an empty list into
    // the document, which the other machine would then apply as a deliberate
    // deletion of every term.
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
            // Or the push sits in memory until some unrelated note edit
            // happens to move the heads. See `SyncDoc::pending_save`.
            sync.mark_pending_save();
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
    /// Both sides already agree with the baseline. Nothing is written and no
    /// lock is taken.
    Idle,
    /// The file moved: put its contents into the document.
    PushToDoc(String),
    /// The document moved: write its contents to the file.
    WriteToFile(String),
}

/// Decide what one pass owes, from the file, the document, and the content
/// both sides agreed on at the end of the previous pass.
///
/// `last` is `None` on the first pass of a run, which reads as "no baseline
/// yet" rather than "empty": a document that already holds a list wins over
/// this machine's file, and an empty one is seeded from it.
///
/// **The document wins whenever it moved**, which is what makes a collision
/// (both sides changed since the baseline) resolve the same way on both
/// machines. Picking the local file instead would have each machine prefer
/// its own copy and the two would ping-pong until one stopped editing. See
/// `docs/plans/2026-08-29-vocabulary-sync-design.md`.
pub fn decide(file: Option<&str>, doc: Option<&str>, last: Option<&str>) -> VocabAction {
    // Guarded rather than unwrapped: a key that vanished from the document is
    // not an instruction to blank the file.
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

    /// A document and a vocabulary path rooted in a PID-scoped temp
    /// directory, so tests never touch the real `%APPDATA%`/`~/.config`
    /// vocabulary and concurrent runs don't race. Mirrors
    /// `config::vocabulary`'s own `temp_vocab` and `lifecycle`'s `temp_store`.
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

    /// The document has to be told it owes a save, or a push would sit in
    /// memory until some unrelated note edit happened to flush it.
    #[test]
    fn pushing_a_local_list_marks_the_document_for_saving() {
        let (mut sync, path) = temp_sync("marks_pending");
        std::fs::write(&path, "alpha").unwrap();

        reconcile(&mut sync).unwrap();

        assert!(sync.has_pending_save());
    }

    /// The baseline's whole job: a second pass over two sides that already
    /// agree must not take a write lock or claim a save is owed. Without it
    /// every 500 ms tick would rewrite the document forever.
    #[test]
    fn a_settled_pass_writes_nothing() {
        let (mut sync, path) = temp_sync("settled");
        std::fs::write(&path, "alpha").unwrap();
        reconcile(&mut sync).unwrap();
        sync.clear_pending_save();

        reconcile(&mut sync).unwrap();

        assert!(!sync.has_pending_save());
    }

    /// `sync_server` opens a document the same way a client does but has no
    /// vocabulary of its own, and must never invent one.
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

    /// The collision rule, and the whole reason `decide` takes three values
    /// rather than two: with both sides changed there is no merge to do, so
    /// one has to be picked, and both machines have to pick the same one or
    /// they ping-pong forever. The document is the corpus's source of truth
    /// everywhere else, so it wins here too.
    #[test]
    fn when_both_sides_changed_the_document_wins() {
        let action = decide(Some("alpha\nlocal"), Some("alpha\nremote"), Some("alpha"));
        assert_eq!(action, VocabAction::WriteToFile("alpha\nremote".to_string()));
    }

    /// First pass of a run: no baseline yet. A document that already holds a
    /// list is the other machine's, and it wins over whatever this machine
    /// happens to have on disk, by the same rule as a collision.
    #[test]
    fn with_no_baseline_a_populated_document_wins() {
        let action = decide(Some("local"), Some("remote"), None);
        assert_eq!(action, VocabAction::WriteToFile("remote".to_string()));
    }

    /// The other half of the first pass: nothing in the document yet, so this
    /// machine's file seeds it. This is what happens on the very first launch
    /// after the feature ships.
    #[test]
    fn with_no_baseline_an_empty_document_is_seeded_from_the_file() {
        let action = decide(Some("alpha\nbeta"), None, None);
        assert_eq!(action, VocabAction::PushToDoc("alpha\nbeta".to_string()));
    }

    /// A machine with no vocabulary file yet and nothing in the document has
    /// nothing to do — it must not write an empty string into the document
    /// and call that a change, which would then land on the other machine as
    /// an incoming edit that wipes its list.
    #[test]
    fn a_machine_with_neither_side_populated_stays_idle() {
        assert_eq!(decide(None, None, None), VocabAction::Idle);
    }

    /// Deleting the last term is a real edit, not an absent file. An empty
    /// file with a populated baseline has to propagate, or a cleared list
    /// silently comes back from the other machine.
    #[test]
    fn clearing_every_term_locally_still_propagates() {
        let action = decide(Some(""), Some("alpha"), Some("alpha"));
        assert_eq!(action, VocabAction::PushToDoc(String::new()));
    }
}
