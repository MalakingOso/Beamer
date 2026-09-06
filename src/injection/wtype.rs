#![cfg(not(target_os = "windows"))]

//! Typing via `wtype` (`zwp_virtual_keyboard_v1`). Works on wlroots-family
//! compositors plus COSMIC with zero setup. GNOME/KDE refuse the protocol,
//! which the availability probe detects so this backend falls through there.

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
        // Empty-string probe binds the protocol without pressing a key:
        // exit 0 where supported, exit 1 on GNOME/KDE, spawn error if missing.
        // Bounded: a wedged helper must not hang the availability check.
        let mut cmd = std::process::Command::new("wtype");
        cmd.arg("");
        match super::clipboard::run_with_timeout(&mut cmd, std::time::Duration::from_secs(5)) {
            Ok(Some(out)) if out.status.success() => Ok(()),
            Ok(Some(out)) => Err(format!(
                "wtype probe failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Ok(None) => Err("wtype probe timed out".into()),
            Err(e) => Err(format!("wtype not found in PATH: {}", e)),
        }
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let sanitized = super::sanitize_for_typing(text);
        if sanitized.is_empty() {
            anyhow::bail!("nothing to type after sanitization");
        }
        // -d 8: Chromium/Electron drop events typed at wtype's default speed.
        // Bounded, scaled to the text: a stuck compositor must fail the
        // injection, not park a spawn_blocking thread forever.
        let timeout = std::time::Duration::from_secs(15)
            + std::time::Duration::from_millis(50 * sanitized.chars().count() as u64);
        let mut cmd = std::process::Command::new("wtype");
        cmd.arg("-d").arg("8").arg("--").arg(&sanitized);
        let output = super::clipboard::run_with_timeout(&mut cmd, timeout)?.ok_or_else(|| {
            anyhow::anyhow!("wtype timed out after {}s", timeout.as_secs())
        })?;
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

/// Paste chord via wtype (last resort on wlroots without ydotool).
pub(crate) fn send_paste_chord(use_shift: bool) -> bool {
    let args: &[&str] = if use_shift {
        &["-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl"]
    } else {
        &["-M", "ctrl", "-k", "v", "-m", "ctrl"]
    };
    let mut cmd = std::process::Command::new("wtype");
    cmd.args(args);
    match super::clipboard::run_with_timeout(&mut cmd, std::time::Duration::from_secs(5)) {
        Ok(Some(out)) if out.status.success() => true,
        Ok(Some(out)) => {
            tracing::debug!(
                "wtype chord failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            false
        }
        Ok(None) => {
            tracing::debug!("wtype chord timed out");
            false
        }
        Err(e) => {
            tracing::debug!("wtype unavailable for chord: {}", e);
            false
        }
    }
}
