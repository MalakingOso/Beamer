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
    /// Extension present on disk and enabled in GNOME.
    Enabled,
    /// Present on disk but disabled.
    Disabled,
    /// Not installed.
    NotInstalled,
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

/// Query GNOME for the extension's current state. Returns `NotInstalled`
/// if the `gnome-extensions` tool isn't on PATH or the UUID isn't listed.
pub fn status() -> Status {
    let out = match Command::new("gnome-extensions")
        .arg("list")
        .arg("--details")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Status::NotInstalled,
    };
    let s = String::from_utf8_lossy(&out.stdout);
    parse_status(&s, EXTENSION_UUID).unwrap_or(Status::NotInstalled)
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
