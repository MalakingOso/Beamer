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

        if find_ydotool_socket().is_none() {
            return Err("ydotoold socket not found — is ydotoold running?".into());
        }

        Ok(())
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        // ydotool only handles ASCII keycodes.
        let ascii_text = super::sanitize_for_typing(text);

        if !ascii_text.is_ascii() {
            anyhow::bail!("Text contains characters outside ydotool's ASCII range");
        }

        // 25 ms delay: faster values drop characters at word boundaries on
        // slower event loops (ydotool default is 20). Hold is the default, stated explicitly.
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

/// Look for executable `name` in `path_str`. Takes the path as a parameter
/// so tests can pass a synthetic PATH.
fn find_executable_in(name: &str, path_str: &std::ffi::OsStr) -> bool {
    use std::os::unix::fs::PermissionsExt;

    for dir in std::env::split_paths(path_str) {
        let candidate = dir.join(name);
        if let Ok(metadata) = std::fs::metadata(&candidate) {
            // `mode & 0o111` approximates `which`'s X_OK check; a false positive
            // just surfaces later as a clearer `ydotool` invocation error.
            if metadata.permissions().mode() & 0o111 != 0 {
                return true;
            }
        }
    }
    false
}

fn is_ydotool_in_path() -> bool {
    match std::env::var_os("PATH") {
        Some(paths) => find_executable_in("ydotool", &paths),
        None => false,
    }
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
    use super::find_executable_in;

    #[test]
    fn test_path_walk_finds_executable() {
        use std::os::unix::fs::PermissionsExt;

        // PID-scoped so concurrent test runs don't race on the same /tmp dir.
        let temp_dir = std::env::temp_dir().join(format!("beamer_test_ydotool_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let executable_path = temp_dir.join("ydotool");
        std::fs::write(&executable_path, "#!/bin/sh\necho test").unwrap();
        std::fs::set_permissions(&executable_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(find_executable_in("ydotool", temp_dir.as_os_str()));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_path_walk_skips_non_executable() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = std::env::temp_dir().join(format!("beamer_test_ydotool_nonexec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let non_exec_path = temp_dir.join("ydotool");
        std::fs::write(&non_exec_path, "#!/bin/sh\necho test").unwrap();
        std::fs::set_permissions(&non_exec_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(!find_executable_in("ydotool", temp_dir.as_os_str()));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_path_walk_nonexistent_path() {
        assert!(!find_executable_in(
            "ydotool",
            std::ffi::OsStr::new("/nonexistent/path")
        ));
    }
}
