//! Applying one fully staged group, the last step of `reconcile`. One blocking
//! call covers stop → swap → write → register → start → record, so a
//! cancelled Dioxus task can never strand the server stopped mid-swap:
//! `spawn_blocking` work runs to completion whatever happens to its awaiter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::state::{ComponentStore, Record};
use super::{archive, llama, Component, Group, GroupPlan, Kind};

/// What staging left ready for a member.
#[derive(Debug)]
pub enum Prepared {
    /// Text or task: the bytes come from the exe.
    Content,
    /// A download already on disk that verified; nothing to move.
    InPlace,
    /// A verified download at this `.part` path.
    File(PathBuf),
    /// An extracted archive at this staging dir.
    Dir(PathBuf),
}

/// Stop the group's server, apply members in plan order, start it, then
/// record. On any failure, swapped dirs are rolled back and the server is
/// restarted on what's left, and nothing is recorded, so the next launch
/// retries the whole group.
pub fn apply_group(plan: &GroupPlan, prepared: &HashMap<&'static str, Prepared>) -> Result<()> {
    stop(plan.group);
    let mut swapped: Vec<PathBuf> = Vec::new();
    let result = plan
        .members
        .iter()
        .try_for_each(|c| apply_member(c, prepared.get(c.id), &mut swapped))
        .and_then(|()| start(plan.group));

    match result {
        Ok(()) => {
            for dest in &swapped {
                archive::commit(dest);
            }
            record(&plan.members);
            Ok(())
        }
        Err(e) => {
            for dest in &swapped {
                if let Err(re) = archive::rollback(dest) {
                    tracing::error!("Rolling back {:?} failed: {:#}", dest, re);
                }
            }
            if let Err(se) = start(plan.group) {
                tracing::warn!("Restarting on the previous files failed too: {:#}", se);
            }
            Err(e)
        }
    }
}

fn apply_member(c: &Component, prepared: Option<&Prepared>, swapped: &mut Vec<PathBuf>) -> Result<()> {
    let dest = c.dest();
    match (c.kind, prepared) {
        (Kind::Archive { .. }, Some(Prepared::Dir(staging))) => {
            archive::swap_in(staging, &dest)?;
            swapped.push(dest);
        }
        (Kind::Text(_), _) => write_atomic(&dest, &c.content().unwrap_or_default())?,
        (Kind::Task { name, .. }, _) => {
            write_atomic(&dest, &c.content().unwrap_or_default())?;
            llama::register_task(name, &dest).with_context(|| format!("registering {name}"))?;
        }
        (Kind::File { .. }, Some(Prepared::File(part))) => {
            if let Some(dir) = dest.parent() {
                std::fs::create_dir_all(dir)?;
            }
            crate::notes::sync_doc::rename_with_retry(part, &dest)
                .with_context(|| format!("moving {:?} into place", part))?;
        }
        (Kind::File { .. }, Some(Prepared::InPlace)) => {}
        _ => bail!("{} was not staged", c.id),
    }
    Ok(())
}

fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = crate::notes::sync_doc::rename_with_retry(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {:?}", dest));
    }
    Ok(())
}

fn stop(group: Option<Group>) {
    match group {
        Some(Group::LlamaServer) => llama::stop_server(),
        None => {}
    }
}

fn start(group: Option<Group>) -> Result<()> {
    match group {
        Some(Group::LlamaServer) => llama::start_server(),
        None => Ok(()),
    }
}

/// Only after the group applied and started. A failed save just means the
/// next launch redoes the group, so it's logged rather than failing it.
fn record(members: &[&Component]) {
    let mut store = ComponentStore::load_from(ComponentStore::default_path());
    for c in members {
        store.set(c.id, Record { version: c.version.to_string(), sha256: c.sha256() });
    }
    if let Err(e) = store.save() {
        tracing::error!("Components applied but not recorded ({:#}); the next launch will redo them", e);
    }
}
