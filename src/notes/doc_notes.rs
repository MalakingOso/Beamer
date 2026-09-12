//! Mapping between `Vec<Note>` and the `notes` root of the automerge document.
//! `reconcile` pushes the vec in as a diff; `hydrate` reads it back after a merge.
//! A map keyed by note id (not a list), so concurrent creations on two machines merge.

use anyhow::Result;
use automerge::transaction::Transactable;
use automerge::{AutoCommit, ObjId, ReadDoc};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::model::{Note, NoteColor, NoteOrigin, StageState};
use super::sync_doc::{
    child_map, get_bool, get_str, get_text, put_bool, put_str, put_text, retain_keys, SyncDoc,
    NOTES_KEY,
};

/// A hydrate's result: readable notes, plus ids `reconcile` must keep.
/// A read failure is not proof the note is gone; pruning it would delete it.
pub struct Hydrated {
    pub notes: Vec<Note>,
    pub unreadable: Vec<String>,
}

/// Push `notes` into the document. Writes are guarded on the stored value,
/// so an unchanged tick produces no operations. Unreadable ids are kept.
pub fn reconcile(sync: &mut SyncDoc, notes: &[Note], unreadable: &[String]) -> Result<()> {
    let root = sync.root_map(NOTES_KEY)?;
    let doc = sync.doc_mut();

    let mut keep: Vec<String> = notes.iter().map(|n| n.id.clone()).collect();
    keep.extend_from_slice(unreadable);
    retain_keys(doc, &root, &keep)?;

    for note in notes {
        let obj = child_map(doc, &root, &note.id)?;
        put_str(doc, &obj, "id", &note.id)?;
        put_str(doc, &obj, "created", &note.created)?;
        put_str(doc, &obj, "modified", &note.modified)?;
        put_str(doc, &obj, "raw", &note.raw)?;
        put_text(doc, &obj, "body", &note.body)?; // character-merged, see `sync_doc::put_text`
        merge_stage(doc, &obj, "extract_state", note.extract_state)?;
        put_str(doc, &obj, "origin", &enum_name(&note.origin))?;
        put_str(doc, &obj, "color", &enum_name(&note.color))?;
        put_bool(doc, &obj, "archived", note.archived)?;
        // Attachments were removed: drop the map left behind by older
        // documents so it does not linger in the shared corpus.
        if doc.get(&obj, "attachments")?.is_some() {
            doc.delete(&obj, "attachments")?;
        }
        // The cleanup pass was removed: drop the stage key left behind by
        // older documents for the same reason.
        if doc.get(&obj, "clean_state")?.is_some() {
            doc.delete(&obj, "clean_state")?;
        }
    }
    Ok(())
}

/// Read the document's notes back out, oldest first (`created`, then id).
/// Entries missing required fields are reported in `unreadable`, never invented.
pub fn hydrate(sync: &SyncDoc) -> Hydrated {
    let Some(root) = sync.root_map_if_present(NOTES_KEY) else {
        return Hydrated { notes: Vec::new(), unreadable: Vec::new() };
    };
    let doc = sync.doc();
    let mut notes: Vec<Note> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    for key in doc.keys(&root).collect::<Vec<_>>() {
        let Ok(Some((value, obj))) = doc.get(&root, key.as_str()) else {
            unreadable.push(key);
            continue;
        };
        if !value.is_object() {
            unreadable.push(key);
            continue;
        }
        match read_note(doc, &obj, &key) {
            Some(note) => notes.push(note),
            None => {
                tracing::warn!("Cannot read note {key} out of the sync document; keeping it");
                unreadable.push(key);
            }
        }
    }
    notes.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    Hydrated { notes, unreadable }
}

/// Reconcile one stage field toward `Done`, never away from it. A completed
/// pass is monotonic progress; `Failed`/`Pending` are transient, so plain
/// last-write-wins would let a concurrent failure regress a success. `Skipped`
/// is a deliberate user choice and always propagates, as does `Done` itself.
fn merge_stage(doc: &mut AutoCommit, obj: &ObjId, key: &str, local: StageState) -> Result<()> {
    if !matches!(local, StageState::Done | StageState::Skipped) {
        if let Some(current) = get_str(doc, obj, key) {
            if current == enum_name(&StageState::Done) {
                return Ok(());
            }
        }
    }
    put_str(doc, obj, key, &enum_name(&local))
}

fn read_note(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<Note> {
    let id = get_str(doc, obj, "id").unwrap_or_else(|| key.to_string());
    Some(Note {
        id,
        created: get_str(doc, obj, "created")?,
        modified: get_str(doc, obj, "modified")?,
        raw: get_str(doc, obj, "raw")?,
        body: get_text(doc, obj, "body")?,
        extract_state: read_enum::<StageState>(doc, obj, "extract_state"),
        origin: read_enum::<NoteOrigin>(doc, obj, "origin"),
        color: read_enum_or(doc, obj, "color", NoteColor::Purple),
        archived: get_bool(doc, obj, "archived").unwrap_or(false),
    })
}

/// The serde name of a unit enum, matching the `notes.json` spelling.
fn enum_name<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn read_enum<T: DeserializeOwned + Default>(doc: &AutoCommit, obj: &ObjId, key: &str) -> T {
    read_enum_or(doc, obj, key, T::default())
}

fn read_enum_or<T: DeserializeOwned>(doc: &AutoCommit, obj: &ObjId, key: &str, fallback: T) -> T {
    get_str(doc, obj, key)
        .and_then(|s| serde_json::from_value::<T>(serde_json::Value::String(s)).ok())
        .unwrap_or(fallback)
}
