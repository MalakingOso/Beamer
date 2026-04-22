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
    let lower = app_id.to_ascii_lowercase();
    TERMINAL_APP_IDS.iter().any(|id| *id == lower)
}

/// Returns the focused window's app id, lowercased. Returns `None` if the
/// extension isn't installed/enabled, the D-Bus call fails, or no window
/// is focused. Every failure path is silent (debug-logged) so callers can
/// treat `None` as "unknown, fall back to defaults".
pub fn focused_app_id() -> Option<String> {
    None // filled in later tasks
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
}
