//! Mapping between `Vec<Note>` and the `notes` root of the automerge document.
//!
//! The vec stays the in-memory source of truth. This file is the only place
//! that knows how a note is laid out inside the document, and it runs in two
//! directions: `reconcile` pushes the vec's current state into the document as
//! a diff, and `hydrate` reads a document back out into a vec after a merge.
//!
//! Layout is a map keyed by note id rather than a list. Concurrent creations
//! on two machines then land under two keys and merge without either one
//! having to guess at a list position, and there is no way for the same note
//! to appear twice.

use anyhow::Result;
use automerge::{AutoCommit, ObjId, ReadDoc};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::model::{Attachment, Note, NoteColor, NoteOrigin, StageState};
use super::sync_doc::{
    child_map, get_bool, get_str, get_text, put_bool, put_str, put_text, retain_keys, SyncDoc,
    NOTES_KEY,
};

/// Push `notes` into the document.
///
/// Every write is guarded on the stored value, so a tick where nothing
/// changed produces no operations at all and the document stops growing.
pub fn reconcile(sync: &mut SyncDoc, notes: &[Note]) -> Result<()> {
    let root = sync.root_map(NOTES_KEY)?;
    let doc = sync.doc_mut();

    let keep: Vec<String> = notes.iter().map(|n| n.id.clone()).collect();
    retain_keys(doc, &root, &keep)?;

    for note in notes {
        let obj = child_map(doc, &root, &note.id)?;
        put_str(doc, &obj, "id", &note.id)?;
        put_str(doc, &obj, "created", &note.created)?;
        put_str(doc, &obj, "modified", &note.modified)?;
        put_str(doc, &obj, "raw", &note.raw)?;
        // The one field that merges character by character. See
        // `sync_doc::put_text`.
        put_text(doc, &obj, "body", &note.body)?;
        put_str(doc, &obj, "clean_state", &enum_name(&note.clean_state))?;
        put_str(doc, &obj, "extract_state", &enum_name(&note.extract_state))?;
        put_str(doc, &obj, "origin", &enum_name(&note.origin))?;
        put_str(doc, &obj, "color", &enum_name(&note.color))?;
        put_bool(doc, &obj, "archived", note.archived)?;
        reconcile_attachments(doc, &obj, &note.attachments)?;
    }
    Ok(())
}

/// Read the document's notes back out, oldest first.
///
/// Order comes from `created`, with the id breaking ties. `notes.json` has
/// always been append-only and therefore creation-ordered, so this reproduces
/// the order the file already had, and it stays stable across machines where
/// a map's key order would not.
///
/// An entry missing the fields a note cannot do without is dropped with a
/// warning rather than filled in with invented values. Nothing writes such an
/// entry; one appearing means the document was edited by something else.
pub fn hydrate(sync: &SyncDoc) -> Vec<Note> {
    let Some(root) = sync.root_map_if_present(NOTES_KEY) else {
        return Vec::new();
    };
    let doc = sync.doc();
    let mut notes: Vec<Note> = Vec::new();
    for key in doc.keys(&root).collect::<Vec<_>>() {
        let Ok(Some((value, obj))) = doc.get(&root, key.as_str()) else { continue };
        if !value.is_object() {
            continue;
        }
        match read_note(doc, &obj, &key) {
            Some(note) => notes.push(note),
            None => tracing::warn!("Skipping malformed note {key} in the sync document"),
        }
    }
    notes.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    notes
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

/// Attachments are stored as a map keyed by attachment id, each value the
/// attachment serialized to JSON.
///
/// Per-attachment rather than per-field, because an attachment is added or
/// removed far more often than it is edited, and keying by id means two
/// machines attaching different files to one note both keep theirs. The
/// nested shape (`Location`, and its `Owned`/`External` split) then stays
/// defined in exactly one place, `model.rs`, which the `task_eval` binary
/// includes and which must never learn about automerge.
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

/// Read a note's attachments back, ordered by id.
///
/// `Note::attachments` is documented as being in no particular order (`body`
/// owns reading order through its tokens), and attachment ids come from
/// `next_id`, whose hex millisecond prefix sorts the same way it counts. So
/// id order is insertion order in practice and stable everywhere.
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

/// The serde name of a unit enum, so the document spells `clean_state` the
/// same way `notes.json` does and the mapping lives in `model.rs` alone.
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
