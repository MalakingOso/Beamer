//! The automerge document behind the note and task corpus.
//!
//! One document holds both roots, at `<config_dir>/sync/notes.automerge`.
//! `NoteStore` and `TaskStore` keep their own `Vec`s as the in-memory source
//! of truth; this file is where those vecs are persisted, and where a copy of
//! the same document arriving from another machine gets merged in.
//!
//! Why automerge rather than the JSON the stores used to write: `merge()` is
//! deterministic however the bytes arrived, and an `ObjType::Text` field
//! merges edits character by character, so two machines that both edited the
//! same note offline keep both edits instead of one silently winning.
//!
//! ⚠️ **The document is written from one place, `notes::flush::flush_stores`.**
//! Both stores reconcile into it before any merge happens, because a merge
//! that lands while one store's vec is still stale would be reverted by that
//! store's next reconcile. Adding a second writer reintroduces that race.

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

/// The first change of every Beamer document, byte for byte the same on every
/// machine.
///
/// ⚠️ **Without this, two machines lose one machine's entire corpus on their
/// first merge, better than half the time.** A document that creates its own
/// root maps does so with its own random actor, so two independently created
/// documents hold two different `Map` objects at `ROOT["notes"]`. Merging them
/// leaves both objects there as a conflict; `ReadDoc::get` returns one winner
/// whole, everything inside the loser becomes unreachable, and `reconcile`
/// then prunes against the winner alone. The winner is deterministic, so the
/// same side loses on both machines and its board and its mirror both go
/// empty. Measured against the test as it now stands:
/// `a_fresh_install_that_has_already_saved_still_sees_an_incoming_corpus`
/// failed 50 of 50 runs with this constant taken out of `new_document`. An
/// earlier draft of that test asserted only the incoming corpus, not the
/// laptop's own note, and failed 24 of 40, which is the coin flip you would
/// expect when either side losing shows up half the time.
///
/// Starting every document from one shared change gives both root maps the
/// same object id everywhere, so independent documents write into the *same*
/// map and merge per note. Regenerate with the ignored
/// `regenerate_the_genesis_document` test in `sync_tests.rs`;
/// `the_genesis_document_still_has_the_object_ids_everything_depends_on` fails
/// loudly if an automerge upgrade changes the encoding.
const GENESIS: &[u8] = include_bytes!("genesis.automerge");

/// The actor that authored [`GENESIS`]. Sixteen zero bytes, and no machine
/// ever writes as this actor: every document is re-actored to a random id the
/// moment it is loaded. Only [`build_genesis`] needs it, so it lives under
/// the same `cfg`.
#[cfg(test)]
pub const GENESIS_ACTOR: [u8; 16] = [0; 16];

/// A document holding nothing but the genesis change, with a fresh random
/// actor of its own.
///
/// Used for every new document and by the test that regenerates the bytes.
/// `AutoCommit::load` mints a random actor already, and the explicit
/// `set_actor` here is what keeps the genesis actor from ever writing again.
pub fn new_document() -> AutoCommit {
    let mut doc = AutoCommit::load(GENESIS).expect("the genesis document is built into the binary");
    doc.set_actor(ActorId::random());
    doc
}

/// Build the genesis change from scratch. Deterministic: a fixed actor, a
/// fixed timestamp, and two `put_object` calls in a fixed order.
///
/// Only the regeneration test and the test that guards the encoding call
/// this. Everything else loads [`GENESIS`], because two machines calling this
/// would still be two separate authorships if the bytes were not shared.
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

/// The document, plus what is needed to tell our own last write apart from
/// somebody else's.
#[derive(Debug)]
pub struct SyncDoc {
    doc: AutoCommit,
    path: PathBuf,
    /// The document file's mtime as of our own last write. A file whose mtime
    /// has moved past this carries changes we have not seen.
    last_write: Option<SystemTime>,
    /// Whether the file was already on disk when this document was opened.
    /// Drives the one-time seed from the legacy JSON: a machine that received
    /// the document through sync must never re-seed from its own stale JSON.
    existed: bool,
    /// A file that is there but could not be read, and could not be
    /// quarantined either, latches this and no write ever goes out.
    ///
    /// Without it, an `fs::read` that fails on an existing file (a permission
    /// change, an I/O error, a race on the synced mount Task 10 puts this on)
    /// would leave an empty in-memory document that the very next flush
    /// writes over the intact bytes. The corpus would be gone, with no
    /// `.corrupt` copy, because nothing failed to *parse*. The parse arm has
    /// always quarantined first; this is the arm that cannot.
    read_only: bool,
    /// A mutation landed on this document from somewhere other than
    /// `flush::run_document_pass`'s own reconcile-and-merge, and still owes a
    /// save.
    ///
    /// Task 10's live-sync coroutine calls `receive_sync_message` directly on
    /// the shared document, outside any flush tick. `run_document_pass`
    /// decides whether a tick has anything to save by comparing heads before
    /// and after its own reconcile/merge, a comparison that is blind to a
    /// mutation that happened *before* that tick even started, because the
    /// "before" snapshot is taken fresh each call and already includes it.
    /// Without this flag, a change applied between two ticks would sit in
    /// memory, correctly reflected in the note/task signals, and never reach
    /// disk. See `SyncDoc::mark_pending_save` and `has_pending_save`.
    pending_save: bool,
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
        }
    }
}

/// A shared handle on the document.
///
/// Both stores hold a clone of the same handle so one document can carry both
/// roots while the stores stay separate signals with separate lifetimes.
/// Cloning shares; it never copies the document.
#[derive(Debug, Clone, Default)]
pub struct SyncHandle(Arc<Mutex<SyncDoc>>);

impl PartialEq for SyncHandle {
    /// Identity, not content. Two stores sharing one document are equal here;
    /// comparing document bytes on every `PartialEq` would be absurd, and the
    /// derived `PartialEq` on the stores only exists to compare their vecs.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl SyncHandle {
    pub fn new(doc: SyncDoc) -> Self {
        Self(Arc::new(Mutex::new(doc)))
    }

    /// A poisoned lock means another thread panicked mid-edit. The document
    /// is still structurally valid automerge, and refusing to write notes for
    /// the rest of the session would be worse than carrying on, so the guard
    /// is taken either way.
    pub fn lock(&self) -> MutexGuard<'_, SyncDoc> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl SyncDoc {
    /// Open the document at `path`, creating an empty one if there is nothing
    /// there yet.
    ///
    /// The actor id is left to automerge, which mints a fresh random 16-byte
    /// one on every `new` and every `load`. Deriving it from `machine_id`
    /// instead would be weaker: that is 16 bits, and it travels inside
    /// `machine.json`, so copying a config directory to set up the second
    /// machine would hand both installs the same actor. Two histories written
    /// under one actor is the corrupted-merge case, not a conflict.
    ///
    /// Returns the document alongside a message when the file existed but
    /// could not be read as automerge. The caller surfaces that to the status
    /// log: a corrupt corpus replaced by an empty one, silently, is the
    /// failure this whole path exists to stop.
    pub fn open(path: PathBuf) -> (Self, Option<String>) {
        let mut error = None;
        let mut existed = false;
        let mut read_only = false;
        let mut doc = new_document();

        if path.exists() {
            existed = true;
            match std::fs::read(&path) {
                Ok(bytes) => match load_or_salvage(&bytes) {
                    Ok(loaded) => doc = loaded,
                    Err(e) => {
                        // Quarantine before anything can write over it. The
                        // same reasoning as the JSON stores: an unreadable
                        // corpus is recoverable, an overwritten one is not.
                        let backup = path.with_extension("automerge.corrupt");
                        let message = format!(
                            "Notes document at {} could not be read ({e}); preserved as {}",
                            path.display(),
                            backup.display()
                        );
                        tracing::error!("{message}");
                        if let Err(e) = std::fs::rename(&path, &backup) {
                            tracing::error!("Could not preserve the corrupt document: {e}");
                            // The bytes are still sitting at `path` and we
                            // could not move them aside, so writing is off.
                            read_only = true;
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

        let last_write = existed.then(|| mtime(&path)).flatten();
        (Self { doc, path, last_write, existed, read_only, pending_save: false }, error)
    }

    /// Whether writing is off for this session. See the field.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Move an incoming document we could not parse out of the way, so the
    /// save that follows does not overwrite it.
    ///
    /// `merge_incoming` leaves an unparseable file alone on the theory that a
    /// sync client is mid-write and the next tick will find it whole. That
    /// only holds if nothing writes over it in between, and the same flush
    /// goes on to save our own document to the same path. The quarantine name
    /// carries a timestamp so a second bad delivery cannot overwrite the
    /// first, and a sync client that really was mid-write simply delivers it
    /// again.
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
                // The file is gone from `path`, so `file_moved` goes quiet
                // rather than asking for the same failed merge every tick.
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

    /// Whether the file was on disk when this document was opened. The seed
    /// from legacy JSON runs only when it was not.
    pub fn existed(&self) -> bool {
        self.existed
    }

    pub fn heads(&mut self) -> Vec<ChangeHash> {
        self.doc.get_heads()
    }

    /// Record that a change landed on this document from outside the flush
    /// tick's own reconcile/merge, and still owes a save. See the field.
    ///
    /// Called by the live-sync coroutine right after `receive_sync_message`
    /// applies a peer's changes, never by `flush::run_document_pass` itself,
    /// that path already detects its own changes by comparing heads.
    pub fn mark_pending_save(&mut self) {
        self.pending_save = true;
    }

    /// Peek the flag `mark_pending_save` sets, without clearing it.
    /// `run_document_pass` folds this into its own changed-or-not decision so
    /// a save it could not otherwise see still happens on the next tick.
    ///
    /// Deliberately not consuming: a `save()` right after can still fail, and
    /// a version that cleared the flag unconditionally would then have
    /// nothing left to notice the miss on the *next* tick, since that tick's
    /// own heads comparison sees no change either, since the mutation predates it.
    /// `clear_pending_save` is the only thing allowed to turn this back off,
    /// and only the caller who just saved successfully may call it.
    pub fn has_pending_save(&self) -> bool {
        self.pending_save
    }

    /// Clear the flag after a save that actually reached disk. See
    /// `has_pending_save` for why this is a separate step from reading it.
    pub fn clear_pending_save(&mut self) {
        self.pending_save = false;
    }

    /// Whether the file on disk has been written since we last wrote it.
    ///
    /// A stat, so it is cheap enough for the 500 ms tick. A file that has
    /// appeared since we opened counts as moved, which is how a first sync
    /// into a fresh install gets picked up.
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

    /// Read the file and merge it in. Returns whether anything new arrived.
    ///
    /// A file we cannot parse is left alone rather than quarantined: it may
    /// be a half-written copy from a sync client, and the next tick will find
    /// it whole.
    pub fn merge_incoming(&mut self) -> Result<bool> {
        let bytes = std::fs::read(&self.path)?;
        let mut incoming = AutoCommit::load(&bytes)?;
        let added = self.doc.merge(&mut incoming)?;
        Ok(!added.is_empty())
    }

    /// Record the file on disk as seen, without merging it.
    ///
    /// Used after a merge that turned out to bring nothing new. Without it a
    /// sync client rewriting the file with content we already have would keep
    /// `file_moved` true for the rest of the session.
    ///
    /// ⚠️ There is a window here that mtime detection cannot close. A sync
    /// client that rewrites the file between `merge_incoming`'s read and this
    /// stat leaves us recording the newer mtime against the older content,
    /// and the next save then overwrites a delivery we never merged. Task
    /// 11's live sync is what removes the guesswork; until then the exposure
    /// is one flush interval wide and the delivery is re-sent by any client
    /// that notices the file changed under it.
    pub fn mark_seen(&mut self) {
        self.last_write = mtime(&self.path);
    }

    /// Write the document out, replacing the file atomically.
    ///
    /// Temp file plus rename, matching the JSON stores: a crash mid-write
    /// leaves the previous corpus intact rather than a truncated file the
    /// next launch would quarantine.
    ///
    /// A no-op on a read-only document. The bytes at `path` are the user's
    /// only copy of the corpus and we could not read them; writing what we
    /// have instead would destroy them. `NoteStore::flush_if_dirty` holds
    /// back the JSON mirror for the same reason, so a session that starts
    /// this way persists nothing at all beyond machine-local window state.
    pub fn save(&mut self) -> Result<()> {
        if self.path.as_os_str().is_empty() || self.read_only {
            return Ok(());
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = self.doc.save();
        let tmp = self.path.with_extension("automerge.tmp");
        std::fs::write(&tmp, bytes)?;
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        // Read back after the rename, not before: the mtime that matters is
        // the one the file ended up with.
        self.last_write = mtime(&self.path);
        Ok(())
    }

    /// The map at a root key.
    ///
    /// Normally already there, from [`GENESIS`]. The `put_object` fallback is
    /// for a document written before genesis existed, and it is the thing
    /// genesis exists to stop happening twice: a map created here carries this
    /// machine's actor, and two of them at one key conflict rather than merge.
    pub fn root_map(&mut self, key: &str) -> Result<ObjId> {
        if let Some((_, id)) = self.doc.get(ROOT, key)? {
            return Ok(id);
        }
        tracing::warn!("The sync document has no {key} map; creating one, which will not merge \
                        with another machine's");
        Ok(self.doc.put_object(ROOT, key, ObjType::Map)?)
    }

    /// The map at a root key, or `None` when nothing has ever written it.
    pub fn root_map_if_present(&self, key: &str) -> Option<ObjId> {
        match self.doc.get(ROOT, key) {
            Ok(Some((_, id))) => Some(id),
            _ => None,
        }
    }
}

/// Load a document, falling back to `load_unverified_heads` when the strict
/// load rejects it.
///
/// The strict load verifies each change's hash. A file that fails only that
/// check still holds every operation, with its original object ids, so
/// accepting it keeps the character-level merge with the other machine
/// working. Rebuilding from `notes.json` instead would mint fresh object ids
/// for every note, and a merge against a peer that still holds the originals
/// then resolves each field by conflict rather than by splice.
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

/// Read a string property off a map.
pub fn get_str(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<String> {
    match doc.get(obj, key) {
        Ok(Some((value, _))) => value.into_string().ok(),
        _ => None,
    }
}

/// Read a boolean property off a map.
pub fn get_bool(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<bool> {
    match doc.get(obj, key) {
        Ok(Some((value, _))) => value.to_bool(),
        _ => None,
    }
}

/// Read an f64 property off a map. Confidence is the only float in the
/// corpus, and it is stored as one rather than as a formatted string so a
/// value outside 0.0-1.0 survives the round trip exactly as the model gave it.
pub fn get_f64(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<f64> {
    match doc.get(obj, key) {
        Ok(Some((value, _))) => value.to_scalar().and_then(|s| s.to_f64()),
        _ => None,
    }
}

/// The text at a property, if that property holds a text object.
pub fn get_text(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<String> {
    match doc.get(obj, key) {
        Ok(Some((_, id))) => doc.text(&id).ok(),
        _ => None,
    }
}

/// Write a string property, skipping the write when the value already matches.
///
/// Skipping matters for more than speed. An unconditional `put` on every
/// field of every note, twice a second, would grow the document's history
/// without end and make every tick a write.
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

/// Write an optional string property, deleting the key when the value is gone.
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

/// Update a text property in place, so an edit becomes a splice.
///
/// ⚠️ **This is what makes the whole document worth having.** `update_text`
/// diffs the stored text against `value` and turns the difference into splice
/// operations, which merge character by character with a concurrent edit from
/// another machine. Replacing the object, or storing the body as a plain
/// string property, would make it a last-write-wins register and one of the
/// two machines' typing would vanish on merge.
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

/// Delete every key of `map` that is not in `keep`.
pub fn retain_keys(doc: &mut AutoCommit, map: &ObjId, keep: &[String]) -> Result<()> {
    let stale: Vec<String> =
        doc.keys(map).filter(|k| !keep.iter().any(|kept| kept == k)).collect();
    for key in stale {
        doc.delete(map, key.as_str())?;
    }
    Ok(())
}

/// The child map at `key`, created if absent.
pub fn child_map(doc: &mut AutoCommit, obj: &ObjId, key: &str) -> Result<ObjId> {
    if let Some((v, id)) = doc.get(obj, key)? {
        if v.is_object() {
            return Ok(id);
        }
    }
    Ok(doc.put_object(obj, key, ObjType::Map)?)
}
