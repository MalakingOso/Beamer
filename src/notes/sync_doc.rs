//! The automerge document behind the note and task corpus (`notes.automerge`).
//! The stores' `Vec`s stay the in-memory source of truth; this is where they
//! persist and where another machine's copy merges in. Character-level merge
//! keeps both machines' concurrent edits to one note.
//! ⚠️ Written from one place, `notes::flush::flush_stores`: both stores
//! reconcile before any merge, or a stale vec reverts the merge on next tick.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use anyhow::Result;
use automerge::transaction::Transactable;
use automerge::{ActorId, AutoCommit, ChangeHash, ObjType, ReadDoc, ROOT};

use automerge::ObjId;

/// Root key holding the note corpus.
pub const NOTES_KEY: &str = "notes";
/// Root key holding the task corpus.
pub const TASKS_KEY: &str = "tasks";

/// The first change of every Beamer document, byte-identical on every machine.
/// Without it, two fresh documents hold different `Map` objects at one root key;
/// the merge leaves a conflict, one side wins whole, and the loser's corpus is
/// pruned. Shared genesis gives both root maps the same object id, so documents
/// merge per note. Regenerate via the ignored test in `sync_tests.rs`.
const GENESIS: &[u8] = include_bytes!("genesis.automerge");

/// The actor that authored [`GENESIS`]. No machine ever writes as it; every
/// document is re-actored on load.
#[cfg(test)]
pub const GENESIS_ACTOR: [u8; 16] = [0; 16];

/// A document holding nothing but the genesis change, with a fresh random actor.
pub fn new_document() -> AutoCommit {
    let mut doc = AutoCommit::load(GENESIS).expect("the genesis document is built into the binary");
    doc.set_actor(ActorId::random());
    doc
}

/// Build the genesis change from scratch (fixed actor, timestamp, call order).
/// Only the regeneration/encoding tests call this; everything else loads [`GENESIS`].
#[cfg(test)]
pub fn build_genesis() -> Vec<u8> {
    use automerge::transaction::CommitOptions;

    let mut doc = AutoCommit::new();
    doc.set_actor(ActorId::from(&GENESIS_ACTOR[..]));
    doc.put_object(ROOT, NOTES_KEY, ObjType::Map).expect("root map");
    doc.put_object(ROOT, TASKS_KEY, ObjType::Map).expect("root map");
    doc.commit_with(CommitOptions::default().with_time(0).with_message("beamer genesis"));
    doc.save()
}

/// The document, plus what tells our own last write apart from someone else's.
#[derive(Debug)]
pub struct SyncDoc {
    doc: AutoCommit,
    path: PathBuf,
    /// The file's mtime as of our last write; a newer mtime means unseen changes.
    last_write: Option<SystemTime>,
    /// Whether the file was on disk at open. Only a fresh machine seeds from legacy JSON.
    existed: bool,
    /// Latched when the file exists but could not be read or quarantined: no
    /// write ever goes out, or the next flush would overwrite the intact bytes.
    read_only: bool,
    /// A mutation from outside the flush tick's own reconcile/merge still owes
    /// a save (the tick's heads comparison is blind to it). See `mark_pending_save`.
    pending_save: bool,
    /// The last `save()` failed and nothing has succeeded since. Set/cleared
    /// inside `save` so a failed write is retried on the very next tick.
    save_failed: bool,
    /// Where this machine's `vocabulary.txt` is. `None` by default, which keeps
    /// `sync_server` from inventing a vocabulary; only `NoteStore::load_from` opts in.
    vocab_path: Option<PathBuf>,
    /// What file and document last agreed on, for `doc_vocab::decide`. In memory
    /// only: a persisted baseline would outlive the process that earned it.
    last_vocab: Option<String>,
}

impl Default for SyncDoc {
    fn default() -> Self {
        Self {
            doc: new_document(),
            path: PathBuf::new(),
            last_write: None,
            existed: false,
            read_only: false,
            pending_save: false,
            save_failed: false,
            vocab_path: None,
            last_vocab: None,
        }
    }
}

/// A shared handle on the document. Both stores clone it so one document
/// carries both roots. Cloning shares; it never copies the document.
#[derive(Debug, Clone, Default)]
pub struct SyncHandle(Arc<Mutex<SyncDoc>>);

impl PartialEq for SyncHandle {
    /// Identity, not content.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl SyncHandle {
    pub fn new(doc: SyncDoc) -> Self {
        Self(Arc::new(Mutex::new(doc)))
    }

    /// A poisoned lock still yields its guard: the document stays valid
    /// automerge, and losing the session's notes would be worse.
    pub fn lock(&self) -> MutexGuard<'_, SyncDoc> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SyncDoc {
    /// Open the document at `path`, creating an empty one if absent. The actor
    /// id stays automerge-minted: a `machine_id`-derived actor would collide
    /// across machines when a config dir is copied. Returns an error message
    /// for the status log when the file existed but could not be read.
    pub fn open(path: PathBuf) -> (Self, Option<String>) {
        let mut error = None;
        let mut existed = false;
        let mut read_only = false;
        let mut doc = new_document();

        // `try_exists`, not `exists`: `exists` answers `false` for a stat
        // failure too, which would seed a fresh document over real bytes. A
        // stat failure latches read-only instead.
        match path.try_exists() {
            Ok(true) => {
                existed = true;
                match std::fs::read(&path) {
                    Ok(bytes) => match load_or_salvage(&bytes) {
                        Ok(loaded) => doc = loaded,
                        Err(e) => {
                            // Quarantine before anything writes over it: unreadable is
                            // recoverable, overwritten is not.
                            let backup = path.with_extension("automerge.corrupt");
                            let message = format!(
                                "Notes document at {} could not be read ({e}); preserved as {}",
                                path.display(),
                                backup.display()
                            );
                            tracing::error!("{message}");
                            if let Err(e) = std::fs::rename(&path, &backup) {
                                tracing::error!("Could not preserve the corrupt document: {e}");
                                read_only = true; // bytes still at `path`; writing is off
                            } else {
                                existed = false;
                            }
                            error = Some(message);
                        }
                    },
                    Err(e) => {
                        let message = format!(
                            "Notes document at {} exists but could not be read ({e}); \
                             it will not be written to this session",
                            path.display()
                        );
                        tracing::error!("{message}");
                        read_only = true;
                        error = Some(message);
                    }
                }
            }
            Ok(false) => {}
            Err(e) => {
                let message = format!(
                    "Could not tell whether a notes document exists at {} ({e}); \
                     it will not be written to this session",
                    path.display()
                );
                tracing::error!("{message}");
                existed = true;
                read_only = true;
                error = Some(message);
            }
        }

        let last_write = existed.then(|| mtime(&path)).flatten();
        (
            Self {
                doc,
                path,
                last_write,
                existed,
                read_only,
                pending_save: false,
                save_failed: false,
                vocab_path: None,
                last_vocab: None,
            },
            error,
        )
    }

    /// Opt this document into keeping `vocabulary.txt` in step.
    pub fn set_vocab_path(&mut self, path: PathBuf) {
        self.vocab_path = Some(path);
    }

    pub fn vocab_path(&self) -> Option<&Path> {
        self.vocab_path.as_deref()
    }

    pub fn last_vocab(&self) -> Option<&str> {
        self.last_vocab.as_deref()
    }

    /// Record the content the file and the document now agree on.
    pub fn set_last_vocab(&mut self, content: String) {
        self.last_vocab = Some(content);
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Move an unparseable incoming file aside (timestamped, so a second bad
    /// delivery can't overwrite the first) before the save that would overwrite it.
    pub fn quarantine_incoming(&mut self) {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        let backup = self.path.with_extension(format!("automerge.unreadable-{stamp}"));
        match std::fs::rename(&self.path, &backup) {
            Ok(()) => {
                tracing::error!(
                    "An incoming sync document at {} could not be parsed; preserved as {}",
                    self.path.display(),
                    backup.display()
                );
                // Gone from `path`, so `file_moved` goes quiet instead of retrying the merge every tick.
                self.last_write = None;
            }
            Err(e) => tracing::error!("Could not move the unreadable sync document aside: {e}"),
        }
    }

    pub fn doc(&self) -> &AutoCommit {
        &self.doc
    }

    pub fn doc_mut(&mut self) -> &mut AutoCommit {
        &mut self.doc
    }

    /// Whether the file was on disk at open. The legacy-JSON seed runs only when it was not.
    pub fn existed(&self) -> bool {
        self.existed
    }

    pub fn heads(&mut self) -> Vec<ChangeHash> {
        self.doc.get_heads()
    }

    /// Record a change from outside the flush tick that still owes a save.
    /// Called by the live-sync coroutine, never by `flush::run_document_pass`.
    pub fn mark_pending_save(&mut self) {
        self.pending_save = true;
    }

    /// Peek the flag without clearing it. Cleared only by `clear_pending_save`
    /// after a save that reached disk, so a failed save stays visible.
    pub fn has_pending_save(&self) -> bool {
        self.pending_save
    }

    /// Clear the flag after a save that actually reached disk.
    pub fn clear_pending_save(&mut self) {
        self.pending_save = false;
    }

    pub fn has_save_failed(&self) -> bool {
        self.save_failed
    }

    /// Whether the file on disk moved since our last write. A newly appeared
    /// file counts, which is how a first sync into a fresh install is picked up.
    pub fn file_moved(&self) -> bool {
        if self.path.as_os_str().is_empty() {
            return false;
        }
        match (mtime(&self.path), self.last_write) {
            (Some(on_disk), Some(ours)) => on_disk != ours,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Read the file and merge it in. An unparseable file is left alone: it may
    /// be a half-written copy, and the next tick will find it whole.
    pub fn merge_incoming(&mut self) -> Result<bool> {
        let bytes = std::fs::read(&self.path)?;
        let mut incoming = AutoCommit::load(&bytes)?;
        let added = self.doc.merge(&mut incoming)?;
        Ok(!added.is_empty())
    }

    /// Record the file on disk as seen, without merging it. Used after a merge
    /// that brought nothing new, or `file_moved` would stay true forever.
    /// ⚠️ Racy by nature: a rewrite between `merge_incoming`'s read and this
    /// stat records a newer mtime against older content. Live sync (not mtime)
    /// is the real fix; until then the window is one flush interval wide.
    pub fn mark_seen(&mut self) {
        self.last_write = mtime(&self.path);
    }

    /// Write the document out atomically (temp file plus rename). A no-op on a
    /// read-only document: the unreadable bytes at `path` are the only copy.
    /// ⚠️ The temp name carries this process's pid, since `sync_server` may
    /// share this file on the same machine. Sets/clears `save_failed` itself.
    pub fn save(&mut self) -> Result<()> {
        if self.path.as_os_str().is_empty() || self.read_only {
            return Ok(());
        }
        let result = self.write_and_rename();
        self.save_failed = result.is_err();
        result
    }

    fn write_and_rename(&mut self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = self.doc.save();
        let tmp = self.path.with_extension(format!("automerge.tmp.{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        if let Err(e) = rename_with_retry(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        // The mtime that matters is the one the file ended up with.
        self.last_write = mtime(&self.path);
        Ok(())
    }

    /// The map at a root key. The fallback creates it, but a map created here
    /// carries this machine's actor and won't merge — the thing genesis prevents.
    pub fn root_map(&mut self, key: &str) -> Result<ObjId> {
        if let Some((_, id)) = self.doc.get(ROOT, key)? {
            return Ok(id);
        }
        tracing::warn!("The sync document has no {key} map; creating one, which will not merge \
                        with another machine's");
        Ok(self.doc.put_object(ROOT, key, ObjType::Map)?)
    }

    /// The map at a root key, or `None` when nothing ever wrote it.
    pub fn root_map_if_present(&self, key: &str) -> Option<ObjId> {
        match self.doc.get(ROOT, key) {
            Ok(Some((_, id))) => Some(id),
            _ => None,
        }
    }
}

/// Load a document, salvaging via `load_unverified_heads` when hash
/// verification fails. The operations (and their object ids) survive, so the
/// character-level merge with the peer keeps working.
fn load_or_salvage(bytes: &[u8]) -> Result<AutoCommit, automerge::AutomergeError> {
    match AutoCommit::load(bytes) {
        Ok(doc) => Ok(doc),
        Err(strict) => match AutoCommit::load_unverified_heads(bytes) {
            Ok(doc) => {
                tracing::warn!(
                    "The notes document failed verification ({strict}) but its operations \
                     are intact; loading it unverified rather than rebuilding it"
                );
                Ok(doc)
            }
            Err(_) => Err(strict),
        },
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Rename `from` to `to`, retrying 3x ~50ms apart. On Windows a process
/// holding the destination open without `FILE_SHARE_DELETE` fails the rename
/// transiently; shared by every store's atomic `save`.
pub(crate) fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    const ATTEMPTS: u32 = 3;
    const DELAY: std::time::Duration = std::time::Duration::from_millis(50);
    let mut last_err = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(DELAY);
        }
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("the loop above always runs at least once"))
}

pub fn get_str(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<String> {
    match doc.get(obj, key) {
        Ok(Some((value, _))) => value.into_string().ok(),
        _ => None,
    }
}

pub fn get_bool(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<bool> {
    match doc.get(obj, key) {
        Ok(Some((value, _))) => value.to_bool(),
        _ => None,
    }
}

/// Read an f64 property. Stored as a float (not a string) so an out-of-range
/// confidence survives the round trip exactly as the model gave it.
pub fn get_f64(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<f64> {
    match doc.get(obj, key) {
        Ok(Some((value, _))) => value.to_scalar().and_then(|s| s.to_f64()),
        _ => None,
    }
}

pub fn get_text(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<String> {
    match doc.get(obj, key) {
        Ok(Some((_, id))) => doc.text(&id).ok(),
        _ => None,
    }
}

/// Write a string property, skipping when the value matches. An unconditional
/// `put` on every field every tick would grow history without end.
pub fn put_str(doc: &mut AutoCommit, obj: &ObjId, key: &str, value: &str) -> Result<()> {
    if get_str(doc, obj, key).as_deref() == Some(value) {
        return Ok(());
    }
    doc.put(obj, key, value)?;
    Ok(())
}

pub fn put_bool(doc: &mut AutoCommit, obj: &ObjId, key: &str, value: bool) -> Result<()> {
    if get_bool(doc, obj, key) == Some(value) {
        return Ok(());
    }
    doc.put(obj, key, value)?;
    Ok(())
}

pub fn put_f64(doc: &mut AutoCommit, obj: &ObjId, key: &str, value: f64) -> Result<()> {
    if get_f64(doc, obj, key) == Some(value) {
        return Ok(());
    }
    doc.put(obj, key, value)?;
    Ok(())
}

/// Write an optional string property, deleting the key when gone.
pub fn put_opt_str(
    doc: &mut AutoCommit,
    obj: &ObjId,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    match value {
        Some(v) => put_str(doc, obj, key, v),
        None => {
            if doc.get(obj, key)?.is_some() {
                doc.delete(obj, key)?;
            }
            Ok(())
        }
    }
}

/// Update a text property in place so an edit becomes a splice. ⚠️ This is the
/// document's whole point: char-by-char merge. A plain string (or replacing
/// the object) would be last-write-wins and eat one machine's typing.
pub fn put_text(doc: &mut AutoCommit, obj: &ObjId, key: &str, value: &str) -> Result<()> {
    let existing = match doc.get(obj, key)? {
        Some((v, id)) if v.is_object() => Some(id),
        _ => None,
    };
    let id = match existing {
        Some(id) => id,
        None => doc.put_object(obj, key, ObjType::Text)?,
    };
    if doc.text(&id)? == value {
        return Ok(());
    }
    doc.update_text(&id, value)?;
    Ok(())
}

pub fn retain_keys(doc: &mut AutoCommit, map: &ObjId, keep: &[String]) -> Result<()> {
    let stale: Vec<String> =
        doc.keys(map).filter(|k| !keep.iter().any(|kept| kept == k)).collect();
    for key in stale {
        doc.delete(map, key.as_str())?;
    }
    Ok(())
}

pub fn child_map(doc: &mut AutoCommit, obj: &ObjId, key: &str) -> Result<ObjId> {
    if let Some((v, id)) = doc.get(obj, key)? {
        if v.is_object() {
            return Ok(id);
        }
    }
    Ok(doc.put_object(obj, key, ObjType::Map)?)
}
