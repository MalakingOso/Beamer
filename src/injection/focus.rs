#![cfg(not(target_os = "windows"))]

//! Focused-window identification via the Beamer GNOME Shell extension.
//!
//! On GNOME Wayland the compositor does not expose focused-window metadata
//! to unprivileged clients, so we ship a minimal Shell extension that
//! exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` over D-Bus.
//! This module is the client side of that interface.

const TERMINAL_APP_IDS: &[&str] = &[
    "org.gnome.console",
    "org.gnome.terminal",
    "org.wezfurlong.wezterm",
    "dev.warp.warp",
    "com.mitchellh.ghostty",
    "kitty",
    "foot",
    "alacritty",
    "org.kde.konsole",
    "io.elementary.terminal",
    "org.xfce.terminal",
    "com.raggesilver.blackbox",
    "tilix",
];

/// Exact-match against a curated list, case-insensitive. No substring
/// heuristic — "thunderbird" contains "term" but is not a terminal.
pub fn is_terminal(app_id: &str) -> bool {
    TERMINAL_APP_IDS.iter().any(|id| app_id.eq_ignore_ascii_case(id))
}

/// Returns the focused window's app id, lowercased. Returns `None` if the
/// extension isn't installed/enabled, the D-Bus call fails, or no window
/// is focused. Every failure path is silent (debug-logged) so callers can
/// treat `None` as "unknown, fall back to defaults".
pub fn focused_app_id() -> Option<String> {
    None // filled in later tasks
}

fn map_call_result(result: Result<String, zbus::Error>) -> Option<String> {
    match result {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s.to_ascii_lowercase()),
        Err(e) => {
            tracing::debug!("focus: D-Bus call failed: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_app_ids_match_case_insensitive() {
        assert!(is_terminal("org.wezfurlong.wezterm"));
        assert!(is_terminal("Kitty"));
        assert!(is_terminal("FOOT"));
        assert!(is_terminal("org.gnome.Console"));
    }

    #[test]
    fn non_terminal_app_ids_do_not_match() {
        assert!(!is_terminal("firefox"));
        assert!(!is_terminal("code"));
        assert!(!is_terminal("org.mozilla.firefox"));
        assert!(!is_terminal(""));
        assert!(!is_terminal("thunderbird")); // contains "term"; must not match
    }

    #[test]
    fn empty_string_maps_to_none() {
        let got = map_call_result(Ok(String::new()));
        assert_eq!(got, None);
    }

    #[test]
    fn non_empty_string_is_lowercased() {
        let got = map_call_result(Ok("Org.WezFurlong.WezTerm".into()));
        assert_eq!(got.as_deref(), Some("org.wezfurlong.wezterm"));
    }

    #[test]
    fn dbus_error_maps_to_none() {
        let err = zbus::Error::Failure("simulated".into());
        let got = map_call_result(Err(err));
        assert_eq!(got, None);
    }
}
