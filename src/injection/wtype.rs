#![cfg(not(target_os = "windows"))]

//! Text typing via `wtype` (the `zwp_virtual_keyboard_v1` Wayland protocol).
//!
//! Works on wlroots-family compositors (Sway, Hyprland, river, niri, labwc)
//! plus COSMIC — full Unicode with zero setup, because wtype uploads its own
//! keymap. GNOME and KDE both refuse this protocol, which the availability
//! probe detects instantly, so this backend cleanly falls through there.

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;

pub struct WtypeBackend;

impl InjectionBackend for WtypeBackend {
    fn name(&self) -> &'static str {
        "wtype"
    }

    fn display_name(&self) -> &'static str {
        "wtype (Wayland virtual keyboard)"
    }

    fn available(&self) -> Result<(), String> {
        if std::env::var("WAYLAND_DISPLAY").is_err() {
            return Err("not a Wayland session".into());
        }
        // Typing an empty string binds the protocol without pressing any key:
        // exit 0 where the compositor implements virtual-keyboard-v1, exit 1
        // with "Compositor does not support the virtual keyboard protocol"
        // on GNOME/KDE, and a spawn error when wtype isn't installed.
        match std::process::Command::new("wtype").arg("").output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(format!(
                "wtype probe failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Err(e) => Err(format!("wtype not found in PATH: {}", e)),
        }
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let sanitized = super::sanitize_for_typing(text);
        if sanitized.is_empty() {
            anyhow::bail!("nothing to type after sanitization");
        }
        // -d 8: 8 ms between keystrokes. Chromium/Electron apps drop events
        // typed at wtype's default full speed (known upstream bug).
        let output = std::process::Command::new("wtype")
            .arg("-d")
            .arg("8")
            .arg("--")
            .arg(&sanitized)
            .output()?;
        if output.status.success() {
            Ok(InjectionResult {
                method: "wtype".into(),
                target_info: "typed via zwp_virtual_keyboard_v1".into(),
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("wtype failed: {}", stderr.trim())
        }
    }
}

/// Send Ctrl(+Shift)+V via wtype. Last-resort chord mechanism for the
/// clipboard backend on wlroots compositors without ydotool.
pub(crate) fn send_paste_chord(use_shift: bool) -> bool {
    let args: &[&str] = if use_shift {
        &["-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl"]
    } else {
        &["-M", "ctrl", "-k", "v", "-m", "ctrl"]
    };
    match std::process::Command::new("wtype").args(args).output() {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            tracing::debug!(
                "wtype chord failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            false
        }
        Err(e) => {
            tracing::debug!("wtype unavailable for chord: {}", e);
            false
        }
    }
}
