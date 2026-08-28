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
//! the same hash, on the same note or different ones, so removal is
//! refcounted: a file under `attachments_dir` only actually goes away once no
//! attachment anywhere in the store still points at its hash. The user's
//! original file is never touched by any of this; see `model::Attachment`'s
//! doc comment for the fuller version of that guarantee.

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
    /// `None` if `source` cannot be read, in which case the caller keeps
    /// whatever `Location` it already had.
    fn adopt(&self, source: &Path) -> Option<Location> {
        Self::adopt_into(&self.attachments_dir, source)
    }

    /// The logic behind `adopt`, taking the directory explicitly so
    /// `migrate_legacy_attachments` can call it while `self.notes` is
    /// borrowed mutably.
    ///
    /// Idempotent: if a file already sits at the destination (this content
    /// was adopted before, by this attachment or some other one entirely),
    /// it is not rewritten. That idempotency is the whole mechanism behind
    /// "the same bytes attached twice yield one file, one hash".
    fn adopt_into(attachments_dir: &Path, source: &Path) -> Option<Location> {
        let bytes = std::fs::read(source).ok()?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase();
        std::fs::create_dir_all(attachments_dir).ok()?;
        let dest = attachments_dir.join(format!("{hash}.{ext}"));
        if !dest.exists() {
            std::fs::write(&dest, &bytes).ok()?;
        }
        Some(Location::Owned { hash, ext })
    }

    /// Remove `<hash>.<ext>` from `attachments_dir` once nothing in the store
    /// references it any more.
    ///
    /// Content addressing means two attachments, even on different notes, can
    /// share one file on disk; only the last reference's removal actually
    /// deletes it. This only ever touches a path under `attachments_dir`,
    /// Beamer's own copy, never the user's original.
    fn release_attachment_bytes(&self, hash: &str, ext: &str) {
        let still_referenced = self
            .notes
            .iter()
            .flat_map(|n| n.attachments.iter())
            .filter_map(owned_hash)
            .any(|(h, _)| h == hash);
        if still_referenced {
            return;
        }
        let file = self.attachments_dir.join(format!("{hash}.{ext}"));
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

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;
