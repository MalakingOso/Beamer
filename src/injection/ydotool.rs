#![cfg(not(target_os = "windows"))]

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use std::path::PathBuf;

pub struct YdotoolBackend;

impl InjectionBackend for YdotoolBackend {
    fn name(&self) -> &'static str {
        "ydotool"
    }

    fn display_name(&self) -> &'static str {
        "ydotool (kernel uinput)"
    }

    fn available(&self) -> Result<(), String> {
        match std::process::Command::new("which")
            .arg("ydotool")
            .output()
        {
            Ok(out) if out.status.success() => {}
            _ => return Err("ydotool not found in PATH".into()),
        }

        // Check if ydotoold daemon socket exists
        if find_ydotool_socket().is_none() {
            return Err("ydotoold socket not found — is ydotoold running?".into());
        }

        Ok(())
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        // ydotool type only handles ASCII keycodes — transliterate common
        // Unicode punctuation (smart quotes, dashes) that transcription APIs produce
        let ascii_text = transliterate_to_ascii(text);

        if !ascii_text.is_ascii() {
            anyhow::bail!("Text contains characters outside ydotool's ASCII range");
        }

        // ydotool's own defaults are --key-delay=20 and --key-hold=20. We'd been
        // running below that at 12 ms, which drops characters at word boundaries
        // — a short word like "to" followed by " cat" comes out "tocat" because
        // the space arrives while the target app is still processing the prior
        // run. 25 ms is 5 ms above ydotool's default as insurance against slower
        // event loops; --key-hold=20 is the default stated explicitly so it's
        // obvious in the code that we depend on it.
        let output = std::process::Command::new("ydotool")
            .arg("type")
            .arg("--key-delay")
            .arg("25")
            .arg("--key-hold")
            .arg("20")
            .arg("--")
            .arg(&ascii_text)
            .output()?;

        if output.status.success() {
            Ok(InjectionResult {
                method: "ydotool".into(),
                target_info: "via /dev/uinput kernel input".into(),
            })
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ydotool failed: {}", stderr.trim())
        }
    }
}

/// Replace common Unicode punctuation from transcription APIs with ASCII equivalents.
fn transliterate_to_ascii(text: &str) -> String {
    text.replace('\u{2018}', "'")  // left single quote
        .replace('\u{2019}', "'")  // right single quote
        .replace('\u{201C}', "\"") // left double quote
        .replace('\u{201D}', "\"") // right double quote
        .replace('\u{2013}', "-")  // en dash
        .replace('\u{2014}', "--") // em dash
        .replace('\u{2026}', "...") // ellipsis
        .replace('\u{00A0}', " ")  // non-breaking space
}

fn find_ydotool_socket() -> Option<PathBuf> {
    if let Ok(sock) = std::env::var("YDOTOOL_SOCKET") {
        let p = PathBuf::from(&sock);
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(xdg).join(".ydotool_socket");
        if p.exists() {
            return Some(p);
        }
    }

    let uid = unsafe { libc::getuid() };
    let candidates = [
        PathBuf::from(format!("/run/user/{}/.ydotool_socket", uid)),
        PathBuf::from("/tmp/.ydotool_socket"),
    ];

    candidates.into_iter().find(|p| p.exists())
}
