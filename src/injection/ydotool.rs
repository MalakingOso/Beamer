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

        // --key-delay is milliseconds between keypresses. Apps process keyboard
        // input on their event loop (~16 ms per frame for a 60 Hz app); values
        // below ~12 ms reliably drop characters — especially the space following
        // a run of letters, which produces merged words like "endsuppasting".
        // 12 ms gives apps enough headroom without feeling sluggish.
        let output = std::process::Command::new("ydotool")
            .arg("type")
            .arg("--key-delay")
            .arg("12")
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
