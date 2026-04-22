#![cfg(not(target_os = "windows"))]

//! Install / enable / disable helper for the bundled Beamer Focus Helper
//! GNOME Shell extension.

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
/// whitespace-indented key/value lines per extension, e.g.:
///
/// ```text
/// beamer-focus@beamer.app
///   Name: Beamer Focus Helper
///   State: ACTIVE
///   Type: PER_USER
/// ```
///
/// We only care about the `State:` line for our UUID. Returns `None` if the
/// UUID is not mentioned.
fn parse_status(output: &str, uuid: &str) -> Option<Status> {
    let mut in_block = false;
    for line in output.lines() {
        let trimmed = line.trim_end();
        if trimmed == uuid {
            in_block = true;
            continue;
        }
        if in_block {
            if !trimmed.starts_with(' ') && !trimmed.starts_with('\t') {
                // Next extension's UUID line — we left the block without a State.
                return Some(Status::Disabled);
            }
            if let Some(state) = trimmed.trim().strip_prefix("State:") {
                return Some(match state.trim() {
                    "ACTIVE" => Status::Enabled,
                    _ => Status::Disabled,
                });
            }
        }
    }
    if in_block { Some(Status::Disabled) } else { None }
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
}
