#![cfg(not(target_os = "windows"))]

//! Focused-window identification via the Beamer GNOME Shell extension.
//!
//! On GNOME Wayland the compositor does not expose focused-window metadata
//! to unprivileged clients, so we ship a minimal Shell extension that
//! exports `app.beamer.FocusProvider.GetFocusedAppId() -> s` over D-Bus.
//! This module is the client side of that interface.

/// Returns the focused window's app id, lowercased. Returns `None` if the
/// extension isn't installed/enabled, the D-Bus call fails, or no window
/// is focused. Every failure path is silent (debug-logged) so callers can
/// treat `None` as "unknown, fall back to defaults".
pub fn focused_app_id() -> Option<String> {
    None // filled in later tasks
}
