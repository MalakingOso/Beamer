//! Mapping between `Vec<Note>` and the `notes` root of the automerge document.
//! `reconcile` pushes the vec in as a diff; `hydrate` reads it back after a merge.
//! A map keyed by note id (not a list), so concurrent creations on two machines merge.

use anyhow::Result;
use automerge::{AutoCommit, ObjId, ReadDoc};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::model::{Attachment, Note, NoteColor, NoteOrigin, StageState};
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
        put_str(doc, &obj, "clean_state", &enum_name(&note.clean_state))?;
        put_str(doc, &obj, "extract_state", &enum_name(&note.extract_state))?;
        put_str(doc, &obj, "origin", &enum_name(&note.origin))?;
        put_str(doc, &obj, "color", &enum_name(&note.color))?;
        put_bool(doc, &obj, "archived", note.archived)?;
        reconcile_attachments(doc, &obj, &note.attachments)?;
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

fn read_note(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<Note> {
    let id = get_str(doc, obj, "id").unwrap_or_else(|| key.to_string());
    Some(Note {
        id,
        created: get_str(doc, obj, "created")?,
        modified: get_str(doc, obj, "modified")?,
        raw: get_str(doc, obj, "raw")?,
        body: get_text(doc, obj, "body")?,
        clean_state: read_enum::<StageState>(doc, obj, "clean_state"),
        extract_state: read_enum::<StageState>(doc, obj, "extract_state"),
        origin: read_enum::<NoteOrigin>(doc, obj, "origin"),
        color: read_enum_or(doc, obj, "color", NoteColor::Purple),
        attachments: read_attachments(doc, obj),
        archived: get_bool(doc, obj, "archived").unwrap_or(false),
    })
}

/// Attachments as a map keyed by attachment id (JSON values), so two machines
/// attaching different files to one note both keep theirs. The shape stays
/// defined in `model.rs`, which must never learn about automerge.
fn reconcile_attachments(
    doc: &mut AutoCommit,
    note: &ObjId,
    attachments: &[Attachment],
) -> Result<()> {
    let map = child_map(doc, note, "attachments")?;
    let keep: Vec<String> = attachments.iter().map(|a| a.id().to_string()).collect();
    retain_keys(doc, &map, &keep)?;
    for attachment in attachments {
        let json = serde_json::to_string(attachment)?;
        put_str(doc, &map, attachment.id(), &json)?;
    }
    Ok(())
}

/// Read a note's attachments back, ordered by id (insertion order in practice).
fn read_attachments(doc: &AutoCommit, note: &ObjId) -> Vec<Attachment> {
    let Ok(Some((value, map))) = doc.get(note, "attachments") else { return Vec::new() };
    if !value.is_object() {
        return Vec::new();
    }
    let mut keys: Vec<String> = doc.keys(&map).collect();
    keys.sort();
    keys.iter()
        .filter_map(|key| {
            let json = get_str(doc, &map, key)?;
            match serde_json::from_str::<Attachment>(&json) {
                Ok(a) => Some(a),
                Err(e) => {
                    tracing::warn!("Skipping unreadable attachment {key}: {e}");
                    None
                }
            }
        })
        .collect()
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
