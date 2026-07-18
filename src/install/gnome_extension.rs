#![cfg(not(target_os = "windows"))]
#![allow(dead_code)]

//! Install / enable / disable helper for the bundled Beamer Focus Helper
//! GNOME Shell extension.

use std::path::PathBuf;
use std::process::Command;
use anyhow::Result;

pub const EXTENSION_UUID: &str = "beamer-focus@beamer.app";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Extension is ACTIVE in GNOME Shell — the D-Bus interface is live.
    Enabled,
    /// Shell has scanned the extension but it's not currently ACTIVE — either
    /// it's INITIALIZED (just after first login) or explicitly disabled by the
    /// user. Activating it is a single `gnome-extensions enable` call; no
    /// log-out required.
    Disabled,
    /// Files are on disk at the expected path but GNOME Shell doesn't know
    /// about the UUID yet. This is the expected outcome of the very first
    /// install on Wayland — Shell only rescans `~/.local/share/gnome-shell/
    /// extensions/` at login. The user has to log out and back in; after
    /// that the state flips to `Disabled` and a single enable finishes the
    /// install.
    PendingRestart,
    /// No files on disk, not known to GNOME Shell — never installed (or
    /// already cleanly uninstalled).
    NotInstalled,
    /// Extension is ACTIVE but the running (or installed) version is older
    /// than the bundled one — reinstalling picks up new capabilities
    /// (v2 added direct typing + the recording pill).
    UpdateAvailable,
    /// Files on disk are current but GNOME Shell is still running the old
    /// code — Shell only reloads extension code at login.
    UpdatePendingRestart,
}

/// Parses a line from `gnome-extensions list --details`. Output is
/// whitespace-indented key/value lines per extension, with optional blank
/// lines and unindented description continuation text between blocks:
///
/// ```text
/// other-ext@example.com
///   Description: Multi-line description text
///
/// Unindented continuation text here.
///
///   State: ACTIVE
/// beamer-focus@beamer.app
///   State: ACTIVE
/// ```
///
/// A real UUID line is identified by containing `@` and no leading
/// whitespace. Any other unindented text is treated as free-form continuation
/// and ignored. Returns `None` if the UUID is not mentioned.
fn parse_status(output: &str, uuid: &str) -> Option<Status> {
    let mut in_block = false;
    for line in output.lines() {
        let trimmed_end = line.trim_end();
        let is_uuid_line = !line.starts_with(' ')
            && !line.starts_with('\t')
            && trimmed_end.contains('@')
            && !trimmed_end.is_empty();

        if is_uuid_line && trimmed_end == uuid {
            in_block = true;
            continue;
        }
        if is_uuid_line && in_block {
            // Crossed into the next extension's block without finding State:.
            return Some(Status::Disabled);
        }
        if in_block {
            if let Some(state) = trimmed_end.trim_start().strip_prefix("State:") {
                return Some(match state.trim() {
                    "ACTIVE" => Status::Enabled,
                    _ => Status::Disabled,
                });
            }
        }
    }
    if in_block { Some(Status::Disabled) } else { None }
}

/// Where on disk the extension's files live, to copy from at install time.
/// Resolution order:
///   1. `$BEAMER_EXTENSION_DIR` env var (dev / packaging override)
///   2. `<exe_dir>/../share/beamer/extension/beamer-focus@beamer.app/` (FHS-packaged)
///   3. `<exe_dir>/extension/beamer-focus@beamer.app/` (portable / dev cwd)
///   4. `./extension/beamer-focus@beamer.app/` (cargo-run from repo root)
pub fn locate_source_dir() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("BEAMER_EXTENSION_DIR") {
        let p = PathBuf::from(env);
        if p.join("metadata.json").exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let fhs = dir.join("../share/beamer/extension").join(EXTENSION_UUID);
            if fhs.join("metadata.json").exists() {
                return Some(fhs);
            }
            let portable = dir.join("extension").join(EXTENSION_UUID);
            if portable.join("metadata.json").exists() {
                return Some(portable);
            }
        }
    }
    let cwd = PathBuf::from("extension").join(EXTENSION_UUID);
    if cwd.join("metadata.json").exists() {
        return Some(cwd);
    }
    None
}

fn target_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join(".local/share/gnome-shell/extensions")
        .join(EXTENSION_UUID))
}

/// Extract `"version": N` from an extension metadata.json.
fn parse_metadata_version(json: &str) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value.get("version")?.as_u64().map(|v| v as u32)
}

/// Version of the extension bundled with this Beamer build.
pub fn bundled_version() -> Option<u32> {
    let src = locate_source_dir()?;
    let json = std::fs::read_to_string(src.join("metadata.json")).ok()?;
    parse_metadata_version(&json)
}

/// Version of the extension files installed in the user's GNOME dir.
fn installed_version() -> Option<u32> {
    let dst = target_dir().ok()?;
    let json = std::fs::read_to_string(dst.join("metadata.json")).ok()?;
    parse_metadata_version(&json)
}

/// Refine an ACTIVE extension's status with version information. `live` is
/// what the running Shell reports (v1 has no GetVersion method, so a D-Bus
/// error maps to 1); `installed`/`bundled` come from the metadata files.
fn resolve_enabled_status(live: u32, installed: Option<u32>, bundled: Option<u32>) -> Status {
    let Some(bundled) = bundled else {
        return Status::Enabled; // can't compare without the bundled files
    };
    if live >= bundled {
        return Status::Enabled;
    }
    match installed {
        Some(v) if v >= bundled => Status::UpdatePendingRestart,
        _ => Status::UpdateAvailable,
    }
}

/// Query GNOME for the extension's current state, then consult the
/// filesystem to distinguish "never installed" from "installed but GNOME
/// hasn't re-scanned yet" (the Wayland first-install case).
pub fn status() -> Status {
    // Ask GNOME first. If it knows the UUID, its answer is authoritative.
    if let Ok(o) = Command::new("gnome-extensions")
        .arg("list")
        .arg("--details")
        .output()
    {
        if o.status.success() {
            let s = String::from_utf8_lossy(&o.stdout);
            if let Some(state) = parse_status(&s, EXTENSION_UUID) {
                if state == Status::Enabled {
                    let live = crate::injection::gnome::helper_version().unwrap_or(1);
                    return resolve_enabled_status(live, installed_version(), bundled_version());
                }
                return state;
            }
        }
    }
    // GNOME doesn't know the UUID. If our files are already at the target
    // path, install() has run — Shell just hasn't re-scanned. The user has
    // to log out and back in.
    if let Ok(dst) = target_dir() {
        if dst.join("metadata.json").exists() {
            return Status::PendingRestart;
        }
    }
    Status::NotInstalled
}

/// Copy the bundled extension into the user's GNOME extensions dir and
/// enable it. Idempotent — re-running after a prior install updates the
/// files.
pub fn install() -> Result<()> {
    let src = locate_source_dir()
        .ok_or_else(|| anyhow::anyhow!("extension source directory not found; set BEAMER_EXTENSION_DIR"))?;
    let dst = target_dir()?;
    // Clear any stale files from a prior install so the target reflects
    // only what's bundled with this Beamer version.
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(&src)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    let out = Command::new("gnome-extensions")
        .arg("enable")
        .arg(EXTENSION_UUID)
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // On Wayland, GNOME Shell only scans `~/.local/share/gnome-shell/
        // extensions/` at login. The first install of a never-before-seen
        // UUID will ALWAYS hit this: files are in place, but enable reports
        // "Extension ... does not exist". That's not a failure — it's the
        // architectural predicate of Wayland hot-loading. Swallow the
        // specific error so install() returns Ok(); the caller's subsequent
        // status() call will report PendingRestart and the UI prompts the
        // user to log out and back in.
        let looks_like_pending_restart = err.to_ascii_lowercase().contains("does not exist");
        if !looks_like_pending_restart {
            anyhow::bail!("gnome-extensions enable failed: {}", err.trim());
        }
        tracing::info!(
            "install: enable reported UUID unknown — Wayland hot-load limit, user must log out and back in to activate"
        );
    }
    Ok(())
}

/// Activate an already-installed extension that GNOME has scanned but not
/// yet enabled (`Status::Disabled` — typically the INITIALIZED state right
/// after the first post-install login). Kept separate from `install()` to
/// avoid redundant file-copy work when the files are already in place.
pub fn enable_installed() -> Result<()> {
    let out = Command::new("gnome-extensions")
        .arg("enable")
        .arg(EXTENSION_UUID)
        .output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("gnome-extensions enable failed: {}", err.trim());
    }
    Ok(())
}

/// Disable and remove the extension.
pub fn uninstall() -> Result<()> {
    let _ = Command::new("gnome-extensions")
        .arg("disable")
        .arg(EXTENSION_UUID)
        .output();
    let dst = target_dir()?;
    if dst.exists() {
        std::fs::remove_dir_all(&dst)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_version_parses() {
        assert_eq!(parse_metadata_version(r#"{"uuid":"x","version":2}"#), Some(2));
        assert_eq!(parse_metadata_version(r#"{"uuid":"x"}"#), None);
        assert_eq!(parse_metadata_version("not json"), None);
    }

    #[test]
    fn enabled_current_version_stays_enabled() {
        assert_eq!(resolve_enabled_status(2, Some(2), Some(2)), Status::Enabled);
    }

    #[test]
    fn old_live_and_old_files_needs_update() {
        assert_eq!(
            resolve_enabled_status(1, Some(1), Some(2)),
            Status::UpdateAvailable
        );
    }

    #[test]
    fn old_live_but_current_files_needs_relogin() {
        assert_eq!(
            resolve_enabled_status(1, Some(2), Some(2)),
            Status::UpdatePendingRestart
        );
    }

    #[test]
    fn unknown_bundled_version_stays_enabled() {
        assert_eq!(resolve_enabled_status(1, Some(1), None), Status::Enabled);
    }

    #[test]
    fn parse_status_finds_active() {
        let out = "beamer-focus@beamer.app\n  Name: Beamer Focus Helper\n  State: ACTIVE\n  Type: PER_USER\n";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), Some(Status::Enabled));
    }

    #[test]
    fn parse_status_finds_inactive() {
        let out = "beamer-focus@beamer.app\n  Name: Beamer Focus Helper\n  State: INACTIVE\n";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), Some(Status::Disabled));
    }

    #[test]
    fn parse_status_not_listed() {
        let out = "other-ext@example.com\n  Name: Other\n  State: ACTIVE\n";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), None);
    }

    #[test]
    fn parse_status_handles_multiple_extensions() {
        let out = "\
other-ext@example.com
  State: ACTIVE
beamer-focus@beamer.app
  State: INACTIVE
third-ext@example.com
  State: ACTIVE
";
        assert_eq!(parse_status(out, "beamer-focus@beamer.app"), Some(Status::Disabled));
    }

    #[test]
    fn parse_status_handles_description_continuations() {
        let out = "\
other-ext@example.com
  Name: Other Extension
  Description: Multi-line description

You can support my work by sponsoring me on:
- github.com/example

  Path: /home/x/.local/share/gnome-shell/extensions/other-ext@example.com
  State: ACTIVE
beamer-focus@beamer.app
  Name: Beamer Focus Helper
  State: ACTIVE
  Path: /home/x/.local/share/gnome-shell/extensions/beamer-focus@beamer.app
";
        assert_eq!(
            parse_status(out, "beamer-focus@beamer.app"),
            Some(Status::Enabled)
        );
    }

    #[test]
    fn parse_status_ignores_description_text_before_state() {
        // Regression: target extension's own Description continuation must not
        // cause an early exit before we reach its State: line.
        let out = "\
beamer-focus@beamer.app
  Name: Beamer Focus Helper
  Description: Some description

Free-form continuation line without indent.

  Path: /some/path
  State: ACTIVE
";
        assert_eq!(
            parse_status(out, "beamer-focus@beamer.app"),
            Some(Status::Enabled)
        );
    }
}
