#![cfg(not(target_os = "windows"))]

//! Direct text typing via the Beamer GNOME Shell extension (v2).
//!
//! The extension owns a `Clutter.VirtualInputDevice` inside GNOME Shell — the
//! same mechanism as GNOME's on-screen keyboard — so it can type arbitrary
//! Unicode into the focused window with no uinput permissions, no portal
//! dialogs, and no clipboard involvement. This is the only first-class
//! injection path on GNOME Wayland (Mutter implements neither
//! `zwp_virtual_keyboard_v1` nor `ext-data-control-v1`).

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;

/// Helper interface version that provides `TypeText`/`SendPasteChord`.
const REQUIRED_VERSION: u32 = 2;

/// Live version of the running helper extension, or `None` when it isn't
/// active. A v1 extension has no `GetVersion` method — that surfaces as a
/// D-Bus error and maps to `None` here; callers treat it the same as absent
/// because v1 can't type. Never cached across calls — the user can
/// enable/disable/update the extension at runtime, so every injection must
/// re-check, but exactly once (see `GnomeBackend::available`).
pub(crate) fn helper_version() -> Option<u32> {
    match super::focus::call_helper::<_, u32>(100, "GetVersion", ()) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("gnome helper: GetVersion failed: {}", e);
            None
        }
    }
}

/// Send Ctrl(+Shift)+V through the helper's virtual keyboard. Used by the
/// clipboard backend as its preferred chord mechanism. Returns `false` when
/// the helper is unavailable or the call fails.
///
/// Deliberately does NOT pre-check the version with a separate `GetVersion`
/// round-trip: `GnomeBackend::available` already did that once (per
/// injection) if the `gnome` backend is in the configured chain, and this
/// is typically only reached after that same backend's `TypeText` failed
/// for some other reason. Calling `SendPasteChord` directly and treating any
/// error as "can't chord" gets identical fallback behavior — a v1 or absent
/// extension has no `SendPasteChord` method, so the call errors the same way
/// a proactive version check would have short-circuited — without a
/// redundant second `GetVersion` call per paste.
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
        // The extension types ~8 chars per 16 ms tick and replies when done;
        // scale the D-Bus timeout with text length plus generous headroom.
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
