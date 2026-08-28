//! Editing a note's attachments, its size, and its existence.
//!
//! A third `impl NoteStore`, following `lifecycle.rs`. `mod.rs` is at 470 of
//! the project's 500-line limit and has no room; more importantly these are a
//! coherent group — everything that changes what a note *contains* beyond its
//! text, plus the one operation that removes a note for good.
//!
//! Two conventions carried over from `lifecycle.rs`, both load-bearing:
//!
//! - **`set_size` is machine-local.** It lives in `machine.json`, not
//!   `notes.json`, and does not bump `modified` or dirty the note store.
//!   `Resized` fires per frame during a grip drag and once again when the
//!   window maps; bumping the timestamp would churn the board's newest-first
//!   ordering on every mouse move, and writing it into the synced note would
//!   turn a resize into sync churn with no content change.
//! - **A missing id is a no-op that does not dirty either store**, matching
//!   `set_open`'s guard, so an event arriving after a note was deleted costs
//!   nothing.
//!
//! **This is where Beamer's own copies of attachment bytes are made and
//! unmade.** `add_attachment` copies a dropped file's bytes into
//! `attachments_dir`, content-addressed by sha256, before the record ever
//! reaches `notes.json`; `delete`, `remove_attachment` and `prune_attachments`
//! each release their share of that copy afterward. Two attachments can name
//! the same `(hash, ext)` pair, on the same note or different ones, so
//! removal is refcounted: a file under `attachments_dir` only actually goes
//! away once no attachment anywhere in the store still points at that exact
//! pair. The user's original file is never touched by any of this; see
//! `model::Attachment`'s doc comment for the fuller version of that
//! guarantee, and `model::owned_file_name` for why `hash`/`ext` are never
//! trusted enough to build a path from directly.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::blocks;
use super::model::{Attachment, Location};
use super::{Note, NoteStore};

impl NoteStore {
    /// Remember how big a note's window is, in **logical** pixels.
    ///
    /// ⚠️ The caller must divide out `scale_factor()` first: `WindowEvent::Resized`
    /// carries physical pixels while the builder consumed a `LogicalSize`. At
    /// scale 1.0 the two agree, so getting this wrong is invisible on a
    /// 1x display and wrong on every HiDPI one.
    ///
    /// A no-op when the size is unchanged, for the same reason `set_open`
    /// guards: the caller is a window event, not a user edit, and any
    /// `notes.write()` at all notifies every subscriber — re-rendering the note
    /// and the board once per frame of a resize drag. The unchanged-value guard
    /// itself lives in `MachineStore::set_size`; this method's own guard is
    /// only for a missing note.
    pub fn set_size(&mut self, id: &str, size: (u32, u32)) {
        if self.get(id).is_none() {
            return;
        }
        self.machine.set_size(id, size);
    }

    /// Attach something and append its token to the body.
    ///
    /// The record and the token are written together, in one store write. They
    /// are two halves of one fact, and a note carrying one without the other is
    /// the desynchronised state `blocks` renders as literal text.
    ///
    /// **Copies the bytes in first**, if `attachment` still points at an
    /// external path: this is "copy on attach". A file Beamer cannot read at
    /// the moment of attaching (already gone, permissions) is stored as-is,
    /// `External`, and picks up the same "Locate…" recovery a legacy
    /// attachment gets.
    ///
    /// This *is* a user edit, so it does bump `modified` — dropping a photo on
    /// a note is as much a change as typing into it.
    pub fn add_attachment(&mut self, id: &str, mut attachment: Attachment) {
        // Checked before adopting, ahead of writing the record: a
        // stale event arriving after the note was deleted would otherwise
        // still copy bytes into `attachments_dir` for nothing left to
        // reference them, orphaned residue that no refcount ever collects.
        if self.get(id).is_none() {
            return;
        }
        if let Some(Location::External { path }) = attachment.location() {
            let path = path.clone();
            if let Some(owned) = self.adopt(&path) {
                attachment.set_location(owned);
            }
        }
        let token_id = attachment.id().to_string();
        let Some(note) = self.touch(id) else { return };
        note.body = blocks::insert_token(&note.body, &token_id, true);
        note.attachments.push(attachment);
        self.dirty = true;
    }

    /// Detach something: drop the record and the token together.
    ///
    /// If that was the last attachment anywhere in the store referencing this
    /// hash, its file under `attachments_dir` goes with it.
    pub fn remove_attachment(&mut self, id: &str, attachment_id: &str) {
        let released = self
            .get(id)
            .and_then(|n| n.attachments.iter().find(|a| a.id() == attachment_id))
            .and_then(owned_hash);
        let Some(note) = self.touch(id) else { return };
        note.body = blocks::remove_token(&note.body, attachment_id);
        note.attachments.retain(|a| a.id() != attachment_id);
        self.dirty = true;
        if let Some((hash, ext)) = released {
            self.release_attachment_bytes(&hash, &ext);
        }
    }

    /// Repoint an attachment at a file that moved — the "Locate…" gesture.
    ///
    /// Tries to copy the newly picked file in the same way `add_attachment`
    /// does, so a relocated attachment becomes owned rather than staying a
    /// path forever. Releases the old hash afterward if nothing else in the
    /// store still references it.
    ///
    /// Returns whether anything changed. The id and the token stay as they
    /// were, so the attachment keeps its place in reading order.
    pub fn relocate_attachment(&mut self, id: &str, attachment_id: &str, path: PathBuf) -> bool {
        // Checked up front, before adopting: a missing note or attachment, or
        // a link (which has no file to relocate; the UI never offers
        // "Locate…" on a chip), would otherwise still spend a copy into
        // `attachments_dir` that nothing ends up referencing.
        let can_relocate = self
            .get(id)
            .and_then(|n| n.attachments.iter().find(|a| a.id() == attachment_id))
            .is_some_and(|a| !matches!(a, Attachment::Link { .. }));
        if !can_relocate {
            return false;
        }

        let adopted = self.adopt(&path);
        let new_location = adopted.unwrap_or_else(|| Location::External { path: path.clone() });
        let filename = path.file_name().map(|n| n.to_string_lossy().into_owned());

        let Some(note) = self.touch(id) else { return false };
        let Some(att) = note.attachments.iter_mut().find(|a| a.id() == attachment_id) else {
            return false;
        };
        if att.location() == Some(&new_location) {
            return false;
        }
        let old = owned_hash(att);
        att.set_location(new_location);
        if let Some(name) = filename {
            att.set_filename(name);
        }
        self.dirty = true;
        if let Some((hash, ext)) = old {
            self.release_attachment_bytes(&hash, &ext);
        }
        true
    }

    /// Drop `Attachment` records whose token the user deleted from the text,
    /// releasing each one's share of `attachments_dir` as it goes.
    ///
    /// Deleting the token from a textarea is the gesture that orphans a
    /// record, and this is what collects it.
    ///
    /// Returns whether anything was dropped, so a caller on the render path can
    /// avoid a write when there is nothing to do.
    pub fn prune_attachments(&mut self, id: &str) -> bool {
        let Some(note) = Self::find_mut(&mut self.notes, id) else {
            return false;
        };
        let referenced: Vec<String> =
            blocks::referenced_ids(&note.body).into_iter().map(str::to_string).collect();
        let before = note.attachments.len();
        let released: Vec<(String, String)> = note
            .attachments
            .iter()
            .filter(|a| !referenced.iter().any(|r| r == a.id()))
            .filter_map(owned_hash)
            .collect();
        note.attachments.retain(|a| referenced.iter().any(|r| r == a.id()));
        if note.attachments.len() == before {
            return false;
        }
        self.dirty = true;
        for (hash, ext) in released {
            self.release_attachment_bytes(&hash, &ext);
        }
        true
    }

    /// Remove a note outright. Returns whether one was removed.
    ///
    /// **Real deletion, which this store has never had before.** Archive stays
    /// the default gesture and is non-destructive; this is the deliberate act,
    /// offered only on an already-archived note and only behind a confirm.
    ///
    /// The note's rows in `TaskStore` go with it — see
    /// `TaskStore::delete_for_note`, which the caller invokes alongside this.
    /// That is a deliberate exception to "dismissed rows are retained as
    /// labelled negatives": an explicit delete means gone.
    ///
    /// Releases this note's share of every owned attachment's bytes. The
    /// user's original source file is **never** touched by this, whether the
    /// attachment was ever adopted or is still sitting as an external path:
    /// deleting a note only ever removes Beamer's own copy, never the thing
    /// it was copied from.
    pub fn delete(&mut self, id: &str) -> bool {
        let owned: Vec<(String, String)> =
            self.get(id).map(|n| n.attachments.iter().filter_map(owned_hash).collect()).unwrap_or_default();

        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        if self.notes.len() == before {
            return false;
        }
        self.dirty = true;
        // Drop its window state too rather than waiting for the next load's
        // GC pass. No reason to let a deleted note's entry sit in
        // machine.json until the next restart.
        self.machine.remove(id);
        for (hash, ext) in owned {
            self.release_attachment_bytes(&hash, &ext);
        }
        true
    }

    /// Copy a legacy path-based attachment's bytes into `attachments_dir` and
    /// rewrite it to a content-addressed `Location`, for every attachment
    /// whose file still exists. One whose file cannot be found keeps its
    /// `External` location exactly as it was: a broken reference is
    /// recoverable through "Locate…", and dropping it outright would not be.
    ///
    /// Sets `self.dirty` when anything was migrated, the same free-rewrite
    /// mechanism `migrate_legacy_window_state` uses for window state.
    pub(super) fn migrate_legacy_attachments(&mut self) {
        let attachments_dir = self.attachments_dir.clone();
        let mut migrated = false;
        for note in &mut self.notes {
            for attachment in &mut note.attachments {
                let Some(Location::External { path }) = attachment.location() else { continue };
                let path = path.clone();
                if let Some(owned) = Self::adopt_into(&attachments_dir, &path) {
                    attachment.set_location(owned);
                    migrated = true;
                }
            }
        }
        if migrated {
            self.dirty = true;
        }
    }

    /// Try to copy `source`'s bytes into this store's `attachments_dir`.
    /// `None` if `source` cannot be read (or is over `MAX_ADOPTED_BYTES`), in
    /// which case the caller keeps whatever `Location` it already had.
    fn adopt(&self, source: &Path) -> Option<Location> {
        Self::adopt_into(&self.attachments_dir, source)
    }

    /// The logic behind `adopt`, taking the directory explicitly so
    /// `migrate_legacy_attachments` can call it while `self.notes` is
    /// borrowed mutably.
    ///
    /// Idempotent: if a file already sits at the destination (this content
    /// was adopted before, by this attachment or some other one entirely),
    /// the copy just made is discarded rather than overwriting it. That
    /// idempotency is the whole mechanism behind "the same bytes attached
    /// twice yield one file, one hash".
    ///
    /// Copies via a unique temp file inside `attachments_dir` itself, then
    /// renames it to `<hash>.<ext>`, the same tmp-then-rename shape
    /// `NoteStore::save` already uses for `notes.json`. Without it, a crash
    /// or a full disk mid-write could leave `<hash>.<ext>` holding bytes
    /// that do not actually hash to `hash`; the idempotency check above
    /// would then treat that torn file as already-adopted forever, and no
    /// later attach of the same source would ever repair it.
    ///
    /// Streams `source` into the temp file while hashing it in the same
    /// pass, rather than reading the whole file into memory first: a 4 GB
    /// video dropped on a note must not allocate 4 GB before this function
    /// has even decided whether to keep it. `MAX_ADOPTED_BYTES` bounds the
    /// cost further by refusing anything over that size outright, the same
    /// way an unreadable path is refused: left `External`, not adopted.
    fn adopt_into(attachments_dir: &Path, source: &Path) -> Option<Location> {
        let metadata = std::fs::metadata(source).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_ADOPTED_BYTES {
            return None;
        }
        std::fs::create_dir_all(attachments_dir).ok()?;

        let tmp = attachments_dir.join(format!(".tmp-{}", super::next_id()));
        let hash = match Self::stream_copy_and_hash(source, &tmp) {
            Some(hash) => hash,
            None => {
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
        };
        let ext = extension_for(source);
        let name = super::model::owned_file_name(&hash, &ext)?;
        let dest = attachments_dir.join(&name);
        if dest.exists() {
            let _ = std::fs::remove_file(&tmp);
        } else if std::fs::rename(&tmp, &dest).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        Some(Location::Owned { hash, ext })
    }

    /// Streams `source`'s bytes into `dest`, hashing them in the same pass.
    /// Returns the sha256 hex digest, or `None` on any I/O error, in which
    /// case `dest` may exist but be incomplete; the caller removes it.
    fn stream_copy_and_hash(source: &Path, dest: &Path) -> Option<String> {
        let mut input = std::fs::File::open(source).ok()?;
        let mut output = std::fs::File::create(dest).ok()?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            output.write_all(&buf[..n]).ok()?;
            hasher.update(&buf[..n]);
        }
        output.flush().ok()?;
        Some(format!("{:x}", hasher.finalize()))
    }

    /// Remove `<hash>.<ext>` from `attachments_dir` once nothing in the store
    /// references that exact pair any more.
    ///
    /// Content addressing means two attachments, even on different notes, can
    /// share one file on disk; only the last reference's removal actually
    /// deletes it. Matched on `(hash, ext)` together, not `hash` alone: the
    /// same bytes adopted once from `deck.png` and once from an extensionless
    /// source produce `<hash>.png` and `<hash>.bin`, two files, and releasing
    /// one must not be blocked by a reference to the other still standing.
    ///
    /// Refuses to touch anything if `hash`/`ext` are not shaped like a real
    /// digest and extension (`owned_file_name` rejects them): there is
    /// nothing safe to delete under `attachments_dir` on the say-so of two
    /// strings that could be anything, including a former `notes.json`
    /// carrying a hand-edited or maliciously synced traversal. This only
    /// ever touches a path under `attachments_dir`, Beamer's own copy, never
    /// the user's original.
    fn release_attachment_bytes(&self, hash: &str, ext: &str) {
        let still_referenced = self
            .notes
            .iter()
            .flat_map(|n| n.attachments.iter())
            .filter_map(owned_hash)
            .any(|(h, e)| h == hash && e == ext);
        if still_referenced {
            return;
        }
        let Some(name) = super::model::owned_file_name(hash, ext) else {
            tracing::warn!(
                "refusing to remove an attachment with an unrecognised hash/ext ({:?}, {:?})",
                hash, ext
            );
            return;
        };
        let file = self.attachments_dir.join(name);
        if let Err(e) = std::fs::remove_file(&file) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Could not remove orphaned attachment {:?}: {}", file, e);
            }
        }
    }

    /// The attachment a token refers to, if the note still holds its record.
    ///
    /// `None` is the desynchronised case, and it is not an error: `blocks`
    /// renders an unmatched token as literal text so the note fails visibly.
    pub fn attachment<'a>(note: &'a Note, attachment_id: &str) -> Option<&'a Attachment> {
        note.attachments.iter().find(|a| a.id() == attachment_id)
    }
}

/// An attachment's `(hash, ext)`, if it is `Owned`. `None` for a link and for
/// one still sitting at an external, unmigrated path: neither has a shared
/// file in `attachments_dir` to refcount.
fn owned_hash(a: &Attachment) -> Option<(String, String)> {
    match a.location() {
        Some(Location::Owned { hash, ext }) => Some((hash.clone(), ext.clone())),
        _ => None,
    }
}

/// Attachments larger than this are left `External` rather than copied in.
/// Not a product limit, a safety valve: without it, `adopt_into` would read
/// and hash an arbitrarily large file, at whatever cost, before it can even
/// decide whether to keep it. 512 MiB comfortably covers what a sticky note
/// attachment actually is (photos, PDFs, short recordings) while refusing
/// something like a dropped video outright rather than paying its cost.
const MAX_ADOPTED_BYTES: u64 = 512 * 1024 * 1024;

/// The extension `adopt_into` stores an attachment under: `source`'s own
/// extension, lowercased, if it is 1 to 16 ASCII alphanumeric characters
/// (the same shape `model::owned_file_name` requires), otherwise `"bin"`.
/// `Path::extension()` cannot itself contain a separator, but this still
/// guards against whatever else a filename could carry (spaces, unicode,
/// an extension longer than any real one) rather than trusting it into a
/// path unchecked.
fn extension_for(source: &Path) -> String {
    source
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && e.len() <= 16 && e.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".to_string())
}

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;
