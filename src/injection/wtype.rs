#![cfg(not(target_os = "windows"))]

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
        if std::env::var("WAYLAND_DISPLAY").is_err()
            && std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland")
        {
            return Err("Not a Wayland session".into());
        }

        match std::process::Command::new("which")
            .arg("wtype")
            .output()
        {
            Ok(out) if out.status.success() => Ok(()),
            _ => Err("wtype not found in PATH (install wtype)".into()),
        }
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let output = std::process::Command::new("wtype")
            .arg("--")
            .arg(text)
            .output()?;

        if output.status.success() {
            Ok(InjectionResult {
                method: "wtype".into(),
                target_info: "via Wayland virtual keyboard".into(),
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("wtype failed: {}", stderr.trim())
        }
    }
}
