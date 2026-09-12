//! Size and deletion edits. Machine writes here never bump `modified`
//! (`set_size` lives in `machine.json`); a missing id is a no-op that dirties nothing.

use super::NoteStore;

impl NoteStore {
    /// Remember a note's window size in **logical** pixels (caller must divide out
    /// `scale_factor()` first: `Resized` carries physical pixels). Unchanged is a no-op.
    pub fn set_size(&mut self, id: &str, size: (u32, u32)) {
        if self.get(id).is_none() {
            return;
        }
        self.machine.set_size(id, size);
    }

    /// Remove a note outright (archive stays the default gesture). The caller's
    /// `TaskStore::delete_for_note` removes its task rows too.
    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.id != id);
        if self.notes.len() == before {
            return false;
        }
        self.dirty = true;
        // Drop its window state now rather than waiting for the next load's GC.
        self.machine.remove(id);
        true
    }
}

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "edit/delete_tests.rs"]
mod delete_tests;
