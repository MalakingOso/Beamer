//! Mapping between `Vec<Task>` and the `tasks` root of the automerge document.
//!
//! Same shape as `doc_notes`: a map keyed by task id, reconciled from the
//! store's vec and hydrated back after a merge. Task rows are written by the
//! extraction pass and then decided once by the user, so nothing here is
//! character-merged; keying by id is what matters, because it means two
//! machines deciding different rows keep both decisions.

use anyhow::Result;
use automerge::{AutoCommit, ObjId, ReadDoc};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::sync_doc::{
    child_map, get_bool, get_f64, get_str, put_bool, put_f64, put_opt_str, put_str, retain_keys,
    SyncDoc, TASKS_KEY,
};
use super::task::{Task, TaskKind, TaskStatus};

pub fn reconcile(sync: &mut SyncDoc, tasks: &[Task]) -> Result<()> {
    let root = sync.root_map(TASKS_KEY)?;
    let doc = sync.doc_mut();

    let keep: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
    retain_keys(doc, &root, &keep)?;

    for task in tasks {
        let obj = child_map(doc, &root, &task.id)?;
        put_str(doc, &obj, "id", &task.id)?;
        put_str(doc, &obj, "note_id", &task.note_id)?;
        put_str(doc, &obj, "text", &task.text)?;
        put_str(doc, &obj, "evidence", &task.evidence)?;
        put_f64(doc, &obj, "confidence", f64::from(task.confidence))?;
        put_str(doc, &obj, "status", &enum_name(&task.status))?;
        put_bool(doc, &obj, "done", task.done)?;
        put_str(doc, &obj, "created", &task.created)?;
        put_opt_str(doc, &obj, "decided", task.decided.as_deref())?;
        put_opt_str(doc, &obj, "due", task.due.as_deref())?;
        put_bool(doc, &obj, "due_all_day", task.due_all_day)?;
        put_opt_str(doc, &obj, "due_phrase", task.due_phrase.as_deref())?;
        put_str(doc, &obj, "kind", &enum_name(&task.kind))?;
    }
    Ok(())
}

/// Read the document's tasks back out, oldest first.
///
/// `created` then id, the same ordering rule `doc_notes::hydrate` uses and
/// for the same reason: `tasks.json` is append-only, so this is the order the
/// file already had.
pub fn hydrate(sync: &SyncDoc) -> Vec<Task> {
    let Some(root) = sync.root_map_if_present(TASKS_KEY) else {
        return Vec::new();
    };
    let doc = sync.doc();
    let mut tasks: Vec<Task> = Vec::new();
    for key in doc.keys(&root).collect::<Vec<_>>() {
        let Ok(Some((value, obj))) = doc.get(&root, key.as_str()) else { continue };
        if !value.is_object() {
            continue;
        }
        match read_task(doc, &obj, &key) {
            Some(task) => tasks.push(task),
            None => tracing::warn!("Skipping malformed task {key} in the sync document"),
        }
    }
    tasks.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    tasks
}

fn read_task(doc: &AutoCommit, obj: &ObjId, key: &str) -> Option<Task> {
    Some(Task {
        id: get_str(doc, obj, "id").unwrap_or_else(|| key.to_string()),
        note_id: get_str(doc, obj, "note_id")?,
        text: get_str(doc, obj, "text")?,
        evidence: get_str(doc, obj, "evidence").unwrap_or_default(),
        // Deliberately not clamped. A value outside 0.0-1.0 means the prompt
        // or the parser misfired, and that anomaly is what the corpus wants
        // to keep. See `task::Task::confidence`.
        confidence: get_f64(doc, obj, "confidence").unwrap_or_default() as f32,
        status: read_enum::<TaskStatus>(doc, obj, "status"),
        done: get_bool(doc, obj, "done").unwrap_or(false),
        created: get_str(doc, obj, "created")?,
        decided: get_str(doc, obj, "decided"),
        due: get_str(doc, obj, "due"),
        due_all_day: get_bool(doc, obj, "due_all_day").unwrap_or(false),
        due_phrase: get_str(doc, obj, "due_phrase"),
        kind: read_enum::<TaskKind>(doc, obj, "kind"),
    })
}

fn enum_name<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn read_enum<T: DeserializeOwned + Default>(doc: &AutoCommit, obj: &ObjId, key: &str) -> T {
    get_str(doc, obj, key)
        .and_then(|s| serde_json::from_value::<T>(serde_json::Value::String(s)).ok())
        .unwrap_or_default()
}
