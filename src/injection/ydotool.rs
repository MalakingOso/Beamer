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
        if !is_ydotool_in_path() {
            return Err("ydotool not found in PATH".into());
        }

        // Check if ydotoold daemon socket exists
        if find_ydotool_socket().is_none() {
            return Err("ydotoold socket not found — is ydotoold running?".into());
        }

        Ok(())
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        // ydotool type only handles ASCII keycodes — the shared sanitizer
        // transliterates smart punctuation and strips newlines/tabs.
        let ascii_text = super::sanitize_for_typing(text);

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

fn is_ydotool_in_path() -> bool {
    use std::os::unix::fs::PermissionsExt;

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let ydotool_path = dir.join("ydotool");
            if ydotool_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&ydotool_path) {
                    if metadata.permissions().mode() & 0o111 != 0 {
                        return true;
                    }
                }
            }
        }
    }
    false
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

#[cfg(test)]
mod tests {
    /// Helper to test PATH walk logic with a custom path string.
    fn check_executable_in_paths(name: &str, path_str: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;

        for dir in std::env::split_paths(path_str) {
            let exe_path = dir.join(name);
            if exe_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&exe_path) {
                    if metadata.permissions().mode() & 0o111 != 0 {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[test]
    fn test_path_walk_finds_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = std::env::temp_dir().join("beamer_test_ydotool");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create an executable file
        let executable_path = temp_dir.join("ydotool");
        std::fs::write(&executable_path, "#!/bin/sh\necho test").unwrap();
        std::fs::set_permissions(&executable_path, std::fs::Permissions::from_mode(0o755))
            .unwrap();

        assert!(check_executable_in_paths(
            "ydotool",
            &temp_dir.to_string_lossy()
        ));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_path_walk_skips_non_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = std::env::temp_dir().join("beamer_test_ydotool_nonexec");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create a non-executable file
        let non_exec_path = temp_dir.join("ydotool");
        std::fs::write(&non_exec_path, "#!/bin/sh\necho test").unwrap();
        std::fs::set_permissions(&non_exec_path, std::fs::Permissions::from_mode(0o644))
            .unwrap();

        assert!(!check_executable_in_paths(
            "ydotool",
            &temp_dir.to_string_lossy()
        ));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_path_walk_nonexistent_path() {
        assert!(!check_executable_in_paths("ydotool", "/nonexistent/path"));
    }
}
