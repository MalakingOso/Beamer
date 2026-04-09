#![cfg(not(target_os = "windows"))]

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use std::io::Write;

pub struct DotoolBackend;

impl InjectionBackend for DotoolBackend {
    fn name(&self) -> &'static str {
        "dotool"
    }

    fn display_name(&self) -> &'static str {
        "dotool (uinput + XKB layout)"
    }

    fn available(&self) -> Result<(), String> {
        match std::process::Command::new("which")
            .arg("dotool")
            .output()
        {
            Ok(out) if out.status.success() => Ok(()),
            _ => Err("dotool not found in PATH (install dotool)".into()),
        }
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        // Pipe "type <text>" to dotool via stdin to avoid argument escaping issues
        let mut child = std::process::Command::new("dotool")
            .stdin(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            write!(stdin, "type {}", text)?;
        }

        let status = child.wait()?;
        if status.success() {
            Ok(InjectionResult {
                method: "dotool".into(),
                target_info: "via uinput with XKB layout".into(),
            })
        } else {
            anyhow::bail!("dotool exited with status {}", status)
        }
    }
}
