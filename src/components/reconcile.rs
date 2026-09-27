//! The launch-time reconcile, started from `App()` via `use_components`, as a
//! Dioxus task: plan, stage each group (downloads + verification, nothing live
//! touched), then hand a fully staged group to `apply` in one blocking call.
//! Statuses drive the Settings rows (`update_card`, `local_ai_card`).
//!
//! Groups are all-or-nothing. A member that needs a prompt nobody has
//! accepted yet, or a member that fails to stage, makes its whole group wait,
//! so the running server keeps its current, consistent files: a new ini must
//! never go live naming a model that isn't on disk.

use std::collections::{HashMap, HashSet};

use dioxus::core::{spawn_forever, Task};
use dioxus::prelude::*;

use super::apply::{self, Prepared};
use super::{archive, catalog, fetch, Component, ComponentStore, GroupPlan, Kind, OnDisk};

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Needs the user's OK to download (see `Component::prompt`).
    Pending,
    Verifying,
    Downloading { bytes: u64, total: u64 },
    Installing,
    Failed(String),
    Ready,
}

/// Handle to the reconcile. `Copy`, so it threads through props like a signal.
/// Button-started runs use `spawn_forever`: a task spawned from a Settings
/// card's handler would die with the card when the user leaves Settings.
#[derive(Clone, Copy, PartialEq)]
pub struct Components {
    /// Keyed by component id, or by `Group::id` for group-wide outcomes.
    status: Signal<HashMap<&'static str, Status>>,
    accepted: CopyValue<HashSet<&'static str>>,
    task: CopyValue<Option<Task>>,
}

/// Create the handle and start the launch-time reconcile. A fresh install (no
/// `config.toml` before this launch) pre-accepts every prompt, so the model
/// downloads without asking. Not keyed on a missing `components.json`: every
/// upgrading 1.0.x install lacks one too.
pub fn use_components(fresh_install: bool) -> Components {
    let status = use_signal(HashMap::new);
    let accepted = use_hook(|| {
        let prompts = catalog().iter().filter(|c| c.prompt && fresh_install).map(|c| c.id);
        CopyValue::new(prompts.collect::<HashSet<_>>())
    });
    let task = use_hook(|| CopyValue::new(None));
    let components = Components { status, accepted, task };
    use_hook(move || components.run());
    components
}

impl Components {
    pub fn status(&self, id: &str) -> Option<Status> {
        self.status.read().get(id).cloned()
    }

    /// (key, label, download size, status) for every row Settings should
    /// show: pending, downloading or failed. Catalog order, groups last.
    pub fn attention(&self) -> Vec<(&'static str, &'static str, Option<u64>, Status)> {
        let status = self.status.read();
        let needs = |s: &Status| matches!(s, Status::Pending | Status::Downloading { .. } | Status::Failed(_));
        let mut rows: Vec<_> = catalog()
            .iter()
            .filter_map(|c| {
                let s = status.get(c.id).filter(|s| needs(s))?;
                Some((c.id, c.label, c.download_size(), s.clone()))
            })
            .collect();
        let mut groups: Vec<_> = catalog().iter().filter_map(|c| c.group).collect();
        groups.dedup();
        for g in groups {
            if let Some(s) = status.get(g.id()).filter(|s| needs(s)) {
                rows.push((g.id(), g.label(), None, s.clone()));
            }
        }
        rows
    }

    /// (Re)start a reconcile, cancelling any run still in flight.
    pub fn run(mut self) {
        if let Some(previous) = self.task.write().take() {
            previous.cancel();
        }
        let task = spawn_forever(reconcile(self));
        *self.task.write() = Some(task);
    }

    /// The user OKed a prompted download.
    pub fn accept(mut self, id: &'static str) {
        self.accepted.write().insert(id);
        self.run();
    }

    /// Stop an in-flight download. Cancelling the task stops the stream (a
    /// flag alone would not: the next chunk would flip the row straight
    /// back to Downloading). Partial files are deleted: restart, never resume.
    pub fn cancel(mut self) {
        if let Some(task) = self.task.write().take() {
            task.cancel();
        }
        for c in catalog() {
            let dest = c.dest();
            let part = match c.kind {
                Kind::File { .. } => fetch::part_path(&dest),
                Kind::Archive { .. } => archive::zip_part_path(&dest),
                _ => continue,
            };
            let _ = std::fs::remove_file(part);
        }
        let mut accepted = self.accepted.write();
        for (id, s) in self.status.write().iter_mut() {
            if !matches!(s, Status::Downloading { .. } | Status::Verifying) {
                continue;
            }
            let prompted = super::find(id).is_some_and(|c| c.prompt);
            if prompted {
                accepted.remove(id);
                *s = Status::Pending;
            } else {
                *s = Status::Failed("Cancelled".into());
            }
        }
    }

    fn set(&mut self, id: &'static str, s: Status) {
        self.status.write().insert(id, s);
    }

    fn clear(&mut self, id: &'static str) {
        self.status.write().remove(id);
    }
}

async fn reconcile(mut c: Components) {
    let catalog = catalog();
    if catalog.is_empty() {
        return;
    }
    c.status.write().clear();
    let planned = tokio::task::spawn_blocking(move || {
        for comp in catalog.iter().filter(|x| matches!(x.kind, Kind::Archive { .. })) {
            archive::sweep(&comp.dest());
        }
        let state = ComponentStore::load_from(ComponentStore::default_path());
        super::plan(catalog, &state, probe)
    })
    .await;
    let plans = match planned {
        Ok(plans) => plans,
        Err(e) => {
            tracing::warn!("Component check panicked: {e}");
            return;
        }
    };
    for plan in plans {
        run_group(&mut c, plan).await;
    }
}

/// Content kinds are compared byte-for-byte (they're small); downloads only
/// by existence here, since re-hashing 1.1 GB every launch is what the
/// recorded sha256 exists to avoid.
fn probe(c: &Component) -> OnDisk {
    let dest = c.dest();
    match c.content() {
        Some(want) => match std::fs::read(&dest) {
            Ok(have) if have == want => OnDisk::Present,
            Ok(_) => OnDisk::Differs,
            Err(_) => OnDisk::Missing,
        },
        None if dest.exists() => OnDisk::Present,
        None => OnDisk::Missing,
    }
}

async fn run_group(c: &mut Components, plan: GroupPlan) {
    let ids: Vec<&str> = plan.members.iter().map(|m| m.id).collect();
    tracing::info!("Components out of date: {ids:?}");
    let mut prepared: HashMap<&'static str, Prepared> = HashMap::new();

    // 1. Files already on disk verify once; a missing or bad one that needs
    //    a prompt nobody accepted parks the whole group before anything else
    //    is fetched.
    let mut waiting = false;
    for m in plan.members.iter().filter(|m| matches!(m.kind, Kind::File { .. })) {
        match in_place(c, m).await {
            Ok(true) => {
                prepared.insert(m.id, Prepared::InPlace);
            }
            Ok(false) if m.prompt && !c.accepted.read().contains(m.id) => {
                c.set(m.id, Status::Pending);
                waiting = true;
            }
            Ok(false) => {}
            Err(e) => return c.set(m.id, Status::Failed(format!("{e:#}"))),
        }
    }
    if waiting {
        tracing::info!("Components {ids:?} wait for a download to be accepted");
        return;
    }

    // 2. Stage the rest. Still nothing live is touched.
    for m in &plan.members {
        if prepared.contains_key(m.id) {
            continue;
        }
        match stage(c, m).await {
            Ok(p) => {
                prepared.insert(m.id, p);
            }
            Err(e) => {
                tracing::warn!("Staging {} failed: {e:#}", m.id);
                return c.set(m.id, Status::Failed(format!("{e:#}")));
            }
        }
    }

    // 3. Apply, in one blocking call.
    for m in &plan.members {
        c.set(m.id, Status::Installing);
    }
    let members = plan.members.clone();
    let group_key = plan.group.map(|g| g.id());
    let outcome = tokio::task::spawn_blocking(move || apply::apply_group(&plan, &prepared)).await;
    let failure = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(e) => Some(format!("install task panicked: {e}")),
    };
    for m in &members {
        match &failure {
            None => c.set(m.id, Status::Ready),
            Some(_) => c.clear(m.id),
        }
    }
    match (failure, group_key) {
        (None, _) => tracing::info!("Components applied: {ids:?}"),
        (Some(msg), key) => {
            tracing::error!("Applying {ids:?} failed: {msg}");
            c.set(key.unwrap_or(members[0].id), Status::Failed(msg));
        }
    }
}

/// A `File` member's destination exists and matches its sha256.
async fn in_place(c: &mut Components, m: &'static Component) -> anyhow::Result<bool> {
    let Kind::File { sha256, size, .. } = m.kind else { return Ok(false) };
    let dest = m.dest();
    if !dest.exists() {
        return Ok(false);
    }
    c.set(m.id, Status::Verifying);
    let ok = tokio::task::spawn_blocking(move || fetch::verify_against(&dest, size, sha256)).await??;
    c.clear(m.id);
    if !ok {
        tracing::warn!("{} is on disk but fails verification; it will be replaced", m.id);
    }
    Ok(ok)
}

async fn stage(c: &mut Components, m: &'static Component) -> anyhow::Result<Prepared> {
    let dest = m.dest();
    match m.kind {
        Kind::Text(_) | Kind::Task { .. } => Ok(Prepared::Content),
        Kind::File { url, sha256, size } => {
            let part = fetch::part_path(&dest);
            download_verified(c, m, url, sha256, size, &part).await?;
            Ok(Prepared::File(part))
        }
        Kind::Archive { url, sha256, size } => {
            let zip = archive::zip_part_path(&dest);
            download_verified(c, m, url, sha256, size, &zip).await?;
            let staging = archive::staging_path(&dest);
            tokio::task::spawn_blocking(move || {
                if staging.exists() {
                    std::fs::remove_dir_all(&staging)?;
                }
                let extracted = archive::extract(&zip, &staging);
                let _ = std::fs::remove_file(&zip);
                extracted.map(|()| Prepared::Dir(staging))
            })
            .await?
        }
    }
}

async fn download_verified(
    c: &mut Components,
    m: &'static Component,
    url: &str,
    sha256: &'static str,
    size: u64,
    to: &std::path::Path,
) -> anyhow::Result<()> {
    // One status write per whole percent, not per chunk: each write re-renders
    // every subscriber, and a 1.1 GB body arrives in tens of thousands of chunks.
    let mut shown: Option<u64> = None;
    let mut progress = |bytes: u64, total: u64| {
        let pct = bytes.saturating_mul(100).checked_div(total).unwrap_or(0);
        if shown != Some(pct) {
            shown = Some(pct);
            c.set(m.id, Status::Downloading { bytes, total });
        }
    };
    fetch::download(url, size, to, &mut progress).await?;
    c.set(m.id, Status::Verifying);
    let path = to.to_path_buf();
    let ok = tokio::task::spawn_blocking(move || fetch::verify_against(&path, size, sha256)).await??;
    if !ok {
        let _ = std::fs::remove_file(to);
        anyhow::bail!("download failed sha256 verification");
    }
    Ok(())
}
