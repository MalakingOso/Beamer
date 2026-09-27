//! Zip extraction into a staging dir, then a rename swap with the old dir
//! kept as `<dest>.old` until the whole group succeeds. Blocking throughout.

use std::path::{Component as PathPart, Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

fn sibling(dest: &Path, suffix: &str) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

pub fn staging_path(dest: &Path) -> PathBuf {
    sibling(dest, ".staging")
}

pub fn old_path(dest: &Path) -> PathBuf {
    sibling(dest, ".old")
}

/// Where the zip itself is downloaded to, before extraction.
pub fn zip_part_path(dest: &Path) -> PathBuf {
    sibling(dest, ".zip.part")
}

/// Extract `zip` into `into`, which must not exist yet. Rejects the whole
/// archive (and removes the partial dir) on any entry that is absolute or
/// contains `..` anywhere: a runtime zip has no reason for either, and one
/// could write outside `into` (zip-slip).
pub fn extract(zip: &Path, into: &Path) -> Result<()> {
    if into.exists() {
        bail!("{:?} already exists", into);
    }
    std::fs::create_dir_all(into)?;
    let result = extract_into(zip, into);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(into);
    }
    result
}

fn extract_into(zip: &Path, into: &Path) -> Result<()> {
    let file = std::fs::File::open(zip).with_context(|| format!("opening {:?}", zip))?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(name) = safe_name(&entry) else {
            bail!("archive entry {:?} escapes the extraction dir", entry.name());
        };
        let out = into.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut target = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut target)?;
    }
    Ok(())
}

/// `enclosed_name` already refuses absolute paths and anything that climbs
/// out; this also refuses a `..` that climbs back in (`a/../b`).
fn safe_name(entry: &zip::read::ZipFile<'_>) -> Option<PathBuf> {
    let name = entry.enclosed_name()?;
    let clean = name.components().all(|part| matches!(part, PathPart::Normal(_) | PathPart::CurDir));
    clean.then_some(name)
}

/// `staging` → `dest`, parking any current `dest` at `<dest>.old`. On a
/// failed second rename the old dir is put back before returning the error.
pub fn swap_in(staging: &Path, dest: &Path) -> Result<()> {
    let old = old_path(dest);
    if old.exists() {
        std::fs::remove_dir_all(&old).with_context(|| format!("clearing {:?}", old))?;
    }
    let had_old = dest.exists();
    if had_old {
        rename_patiently(dest, &old).with_context(|| format!("moving {:?} aside", dest))?;
    }
    if let Err(e) = rename_patiently(staging, dest) {
        if had_old {
            let _ = rename_patiently(&old, dest);
        }
        return Err(e).with_context(|| format!("moving {:?} into place", staging));
    }
    Ok(())
}

/// The group succeeded: drop the parked old dir. Best-effort, since DLL
/// handles can linger briefly after `taskkill`; `sweep` retries next launch.
pub fn commit(dest: &Path) {
    let old = old_path(dest);
    if old.exists() {
        if let Err(e) = std::fs::remove_dir_all(&old) {
            tracing::info!("Could not remove {:?} yet ({}); next launch will", old, e);
        }
    }
}

/// The group failed after `swap_in`: put the old dir back.
pub fn rollback(dest: &Path) -> Result<()> {
    let old = old_path(dest);
    if !old.exists() {
        return Ok(());
    }
    if dest.exists() {
        std::fs::remove_dir_all(dest).with_context(|| format!("removing {:?}", dest))?;
    }
    rename_patiently(&old, dest).with_context(|| format!("restoring {:?}", old))
}

/// Start-of-reconcile cleanup after an interrupted run. A `.old` with no
/// `dest` means the process died mid-swap: that's the last good dir, so it
/// goes back rather than being deleted.
pub fn sweep(dest: &Path) {
    let old = old_path(dest);
    if old.exists() {
        let outcome = if dest.exists() {
            std::fs::remove_dir_all(&old).map_err(anyhow::Error::from)
        } else {
            rename_patiently(&old, dest).map_err(anyhow::Error::from)
        };
        if let Err(e) = outcome {
            tracing::warn!("Could not tidy {:?}: {}", old, e);
        }
    }
    for leftover in [staging_path(dest), zip_part_path(dest)] {
        let removed = if leftover.is_dir() {
            std::fs::remove_dir_all(&leftover)
        } else {
            std::fs::remove_file(&leftover)
        };
        if let Err(e) = removed {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Could not remove {:?}: {}", leftover, e);
            }
        }
    }
}

/// Renaming a directory on Windows fails with access-denied or a sharing
/// violation while any file in it is still open, which is normal for a
/// moment after `taskkill` returns. `rename_with_retry` gives up after 150ms
/// and only on sharing violations; this waits out either, up to ~3s.
fn rename_patiently(from: &Path, to: &Path) -> std::io::Result<()> {
    const ATTEMPTS: u32 = 20;
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(150));
        }
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("at least one attempt"))
}

#[cfg(test)]
mod tests;
