//! Typed field accessors over one automerge document: the `get_*`/`put_*`
//! family plus the map helpers (`retain_keys`, `child_map`), used by
//! `doc_notes`/`doc_tasks`/`doc_vocab`.
//!
//! Every `put_*` is guarded on the stored value, so reconciling an unchanged
//! vec issues no ops. That guard is also what stops a stale in-memory row from
//! silently undoing a peer's write (Look's `tasks[id].done`); never make it unconditional.

use anyhow::Result;
use automerge::transaction::Transactable;
use automerge::{AutoCommit, ObjId, ObjType, ReadDoc};

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

/// Update a text property in place so an edit becomes a splice: the
/// document's whole point is char-by-char merge. A plain string (or replacing
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

/// Delete every key of `map` not in `keep`. Only ever called on the `notes`
/// and `tasks` maps, so root keys Beamer doesn't own (`ROOT.look`) survive.
pub fn retain_keys(doc: &mut AutoCommit, map: &ObjId, keep: &[String]) -> Result<()> {
    let stale: Vec<String> =
        doc.keys(map).filter(|k| !keep.iter().any(|kept| kept == k)).collect();
    for key in stale {
        doc.delete(map, key.as_str())?;
    }
    Ok(())
}

/// The map at `obj[key]`, created there if the key is absent or holds a scalar.
pub fn child_map(doc: &mut AutoCommit, obj: &ObjId, key: &str) -> Result<ObjId> {
    if let Some((v, id)) = doc.get(obj, key)? {
        if v.is_object() {
            return Ok(id);
        }
    }
    Ok(doc.put_object(obj, key, ObjType::Map)?)
}

