#![cfg(not(target_os = "windows"))]
#![allow(dead_code)]

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
