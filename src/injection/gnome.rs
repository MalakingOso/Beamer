#![cfg(not(target_os = "windows"))]

//! Direct typing via the Beamer GNOME Shell extension (v2).
//!
//! The extension types through a `Clutter.VirtualInputDevice` (same mechanism
//! as the on-screen keyboard): no uinput permissions, no portal dialogs, no
//! clipboard. The only first-class injection path on GNOME Wayland.

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;

/// Helper interface version that provides `TypeText`/`SendPasteChord`.
const REQUIRED_VERSION: u32 = 2;

/// Live helper version, or `None` when absent (v1 has no `GetVersion`,
/// so it maps to `None` — v1 can't type). Re-checked per injection, never cached.
pub(crate) fn helper_version() -> Option<u32> {
    match super::focus::call_helper::<_, u32>(100, "GetVersion", ()) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("gnome helper: GetVersion failed: {}", e);
            None
        }
    }
}

/// Paste chord through the helper's virtual keyboard (clipboard backend's
/// preferred chord). No version pre-check: a v1/absent helper lacks the
/// method, so the call just fails and the backend falls through.
pub(crate) fn send_paste_chord(use_shift: bool) -> bool {
    match super::focus::call_helper::<_, bool>(500, "SendPasteChord", use_shift) {
        Ok(ok) => ok,
        Err(e) => {
            tracing::debug!("gnome helper: SendPasteChord failed: {}", e);
            false
        }
    }
}

pub struct GnomeBackend;

impl InjectionBackend for GnomeBackend {
    fn name(&self) -> &'static str {
        "gnome"
    }

    fn display_name(&self) -> &'static str {
        "GNOME Shell helper (direct typing)"
    }

    fn available(&self) -> Result<(), String> {
        match helper_version() {
            Some(v) if v >= REQUIRED_VERSION => Ok(()),
            Some(v) => Err(format!(
                "helper extension v{} is active but v{} is required — \
                 update it in Settings, then log out and back in",
                v, REQUIRED_VERSION
            )),
            None => Err(
                "helper extension not active — install it in Settings \
                 (GNOME only), then log out and back in"
                    .into(),
            ),
        }
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let sanitized = super::sanitize_for_typing(text);
        if sanitized.is_empty() {
            anyhow::bail!("nothing to type after sanitization");
        }
        // The extension replies when done typing; scale the timeout with text length.
        let timeout_ms = 2000 + 20 * sanitized.chars().count() as u64;
        let ok: bool = super::focus::call_helper(timeout_ms, "TypeText", sanitized)
            .map_err(|e| anyhow::anyhow!("TypeText D-Bus call failed: {}", e))?;
        if !ok {
            anyhow::bail!("helper refused TypeText (another injection in progress?)");
        }
        Ok(InjectionResult {
            method: "gnome".into(),
            target_info: "typed via GNOME Shell virtual keyboard".into(),
        })
    }
}
