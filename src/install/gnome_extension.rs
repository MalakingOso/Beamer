#![cfg(not(target_os = "windows"))]
#![allow(dead_code)]

//! Install/enable/disable helper for the bundled GNOME Shell extension.

use std::path::PathBuf;
use std::process::Command;
use anyhow::Result;

pub const EXTENSION_UUID: &str = "beamer-focus@beamer.app";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// ACTIVE in GNOME Shell; the D-Bus interface is live.
    Enabled,
    /// Scanned but not ACTIVE. One `gnome-extensions enable` fixes it.
    Disabled,
    /// On disk but unknown to Shell: first install on Wayland only rescans at
    /// login, so the user must log out and back in.
    PendingRestart,
    /// Not on disk and unknown to Shell.
    NotInstalled,
    /// ACTIVE but older than the bundled version.
    UpdateAvailable,
    /// Files current but Shell still runs the old code; needs a re-login.
    UpdatePendingRestart,
}

/// Parse `gnome-extensions list --details`: blocks keyed by UUID line (an
/// unindented line containing `@`), each followed by indented `Key: value`
/// lines. Unindented non-UUID text is description continuation; ignored.
/// Returns `None` if the UUID is absent.
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
            // Next block without a State: line; treat as scanned but inactive.
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

/// Extension source dir: `$BEAMER_EXTENSION_DIR`, then FHS, portable, then
/// `./extension/` for cargo-run from the repo root.
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

/// `"version": N` from an extension metadata.json.
fn parse_metadata_version(json: &str) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value.get("version")?.as_u64().map(|v| v as u32)
}

/// Bundled extension version.
pub fn bundled_version() -> Option<u32> {
    let src = locate_source_dir()?;
    let json = std::fs::read_to_string(src.join("metadata.json")).ok()?;
    parse_metadata_version(&json)
}

/// Installed extension version.
fn installed_version() -> Option<u32> {
    let dst = target_dir().ok()?;
    let json = std::fs::read_to_string(dst.join("metadata.json")).ok()?;
    parse_metadata_version(&json)
}

/// Refine an ACTIVE status with versions. `live` comes from Shell (v1 has no
/// GetVersion, so a D-Bus error maps to 1); the rest from metadata files.
fn resolve_enabled_status(live: u32, installed: Option<u32>, bundled: Option<u32>) -> Status {
    let Some(bundled) = bundled else {
        return Status::Enabled; // nothing to compare against
    };
    if live >= bundled {
        return Status::Enabled;
    }
    match installed {
        Some(v) if v >= bundled => Status::UpdatePendingRestart,
        _ => Status::UpdateAvailable,
    }
}

/// Current state: GNOME's answer when it knows the UUID, else the filesystem
/// (distinguishes never-installed from installed-but-not-yet-rescanned).
pub fn status() -> Status {
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
    // Unknown UUID but files in place: installed, awaiting Shell's rescan.
    if let Ok(dst) = target_dir() {
        if dst.join("metadata.json").exists() {
            return Status::PendingRestart;
        }
    }
    Status::NotInstalled
}

/// Copy the bundled extension into the GNOME extensions dir and enable it.
/// Idempotent; re-running updates the files.
pub fn install() -> Result<()> {
    let src = locate_source_dir()
        .ok_or_else(|| anyhow::anyhow!("extension source directory not found; set BEAMER_EXTENSION_DIR"))?;
    let dst = target_dir()?;
    // Clear stale files so the target holds exactly this build's bundle.
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
        // First install on Wayland always reports "does not exist": Shell only
        // scans at login. That means PendingRestart, not failure — swallow it
        // so the caller's next `status()` can say so.
        let looks_like_pending_restart = err.to_ascii_lowercase().contains("does not exist");
        if !looks_like_pending_restart {
            anyhow::bail!("gnome-extensions enable failed: {}", err.trim());
        }
        tracing::info!("install: UUID unknown to Shell — re-login required");
    }
    Ok(())
}

/// Enable an installed-but-disabled extension (no file copy).
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
        // Own Description continuation lines must not end the block early.
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
