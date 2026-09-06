//! Typed field accessors over one automerge document: the `get_*`/`put_*`
//! family plus the map helpers (`retain_keys`, `child_map`). `put_*` writes
//! are guarded on the stored value, so reconciling an unchanged vec issues no
//! ops and a no-op flush saves nothing.

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

