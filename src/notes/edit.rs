//! Attachment, size, and deletion edits. Machine writes here never bump `modified`
//! (`set_size` lives in `machine.json`); a missing id is a no-op that dirties nothing.
//!
//! Attachment bytes are copied into `attachments_dir` content-addressed by sha256 and
//! refcounted on `(hash, ext)`; the user's original is never touched. Once sync is on,
//! unreferenced local bytes are kept — another machine may still reference them.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::blocks;
use super::model::{Attachment, Location};
use super::{Note, NoteStore};

impl NoteStore {
    /// Remember a note's window size in **logical** pixels (caller must divide out
    /// `scale_factor()` first: `Resized` carries physical pixels). Unchanged is a no-op.
    pub fn set_size(&mut self, id: &str, size: (u32, u32)) {
        if self.get(id).is_none() {
            return;
        }
        self.machine.set_size(id, size);
    }

    /// Attach something and append its token. Record and token are written together;
    /// one without the other is the desynchronised state `blocks` renders literally.
    /// Copies external bytes in first ("copy on attach"); an unreadable file stays
    /// `External` with "Locate…" recovery. A user edit, so it bumps `modified`.
    pub fn add_attachment(&mut self, id: &str, mut attachment: Attachment) {
        // Check before adopting, or a stale event would orphan bytes no refcount collects.
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

    /// Detach something: drop the record and the token together, releasing the
    /// bytes if nothing else in the store references them.
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

    /// Repoint an attachment at a moved file (the "Locate…" gesture). Adopts the new
    /// file like `add_attachment`; id and token stay put. Returns whether anything changed.
    pub fn relocate_attachment(&mut self, id: &str, attachment_id: &str, path: PathBuf) -> bool {
        // Check before adopting, or a missing note/link would orphan a copy.
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

    /// Drop records whose token the user deleted from the text. Returns whether
    /// anything was dropped, so a render-path caller can skip the write.
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

    /// Remove a note outright (archive stays the default gesture). The caller's
    /// `TaskStore::delete_for_note` removes its task rows too. Releases owned
    /// bytes; the user's originals are never touched.
    pub fn delete(&mut self, id: &str) -> bool {
        let owned: Vec<(String, String)> =
            self.get(id).map(|n| n.attachments.iter().filter_map(owned_hash).collect()).unwrap_or_default();

        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        if self.notes.len() == before {
            return false;
        }
        self.dirty = true;
        // Drop its window state now rather than waiting for the next load's GC.
        self.machine.remove(id);
        for (hash, ext) in owned {
            self.release_attachment_bytes(&hash, &ext);
        }
        true
    }

    /// Adopt every legacy path-based attachment whose file still exists; a missing
    /// file keeps its `External` location ("Locate…" can still recover it).
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

    /// Copy `source`'s bytes into `attachments_dir`. `None` if unreadable or over
    /// `MAX_ADOPTED_BYTES`; the caller then keeps its existing `Location`.
    fn adopt(&self, source: &Path) -> Option<Location> {
        Self::adopt_into(&self.attachments_dir, source)
    }

    /// Directory taken explicitly so `migrate_legacy_attachments` can call this while
    /// `self.notes` is borrowed mutably. Idempotent (same bytes → one file, one hash);
    /// copies via tmp-then-rename so a torn write can never pass as already-adopted.
    /// Streams while hashing so a large file is never read fully into memory.
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

    /// Stream `source` into `dest`, returning the sha256 hex digest. `None` on I/O
    /// error (`dest` may then be incomplete; the caller removes it).
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

    /// Remove `<hash>.<ext>` once nothing in the store references that exact pair.
    /// Matched on `(hash, ext)` together: the same bytes under two extensions are two
    /// files. Unvalidated shapes are refused outright. A no-op once `sync_enabled`:
    /// this store cannot see other machines' references, and a Syncthing-propagated
    /// delete is unrecoverable — orphaned bytes are cheaper than a vanished image.
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
        if self.sync_enabled {
            tracing::debug!(
                hash, ext, "sync is on; keeping this attachment's bytes even though \
                nothing local references them any more, in case another machine still does"
            );
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
    /// `None` is the desynchronised case, not an error (`blocks` renders it literally).
    pub fn attachment<'a>(note: &'a Note, attachment_id: &str) -> Option<&'a Attachment> {
        note.attachments.iter().find(|a| a.id() == attachment_id)
    }
}

/// An attachment's `(hash, ext)` if `Owned`; `None` otherwise (nothing to refcount).
fn owned_hash(a: &Attachment) -> Option<(String, String)> {
    match a.location() {
        Some(Location::Owned { hash, ext }) => Some((hash.clone(), ext.clone())),
        _ => None,
    }
}

/// Attachments over this are left `External` rather than copied in (safety valve
/// so `adopt_into` never pays an unbounded read+hash before deciding).
const MAX_ADOPTED_BYTES: u64 = 512 * 1024 * 1024;

/// `source`'s own extension, lowercased, if it fits `owned_file_name`'s shape; else `"bin"`.
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
