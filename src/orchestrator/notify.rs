/// Last-resort fallback: just put text on clipboard without sending Ctrl+V.
/// User pastes manually. This avoids all compositor/keyboard protocol issues.
pub(super) async fn clipboard_only_fallback(text: &str) -> anyhow::Result<()> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_text(&text)?;

        // On Wayland, back arboard up with wl-copy
        #[cfg(not(target_os = "windows"))]
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .arg("--type")
                .arg("text/plain")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = stdin.write_all(text.as_bytes());
                    // Dropping stdin closes the pipe so wl-copy forks.
                }
                // Reap the daemonizing parent so it doesn't linger as a zombie.
                crate::injection::clipboard::reap_daemonized(child, "wl-copy");
            }
        }

        Ok(())
    })
    .await?
}

/// Show a desktop notification. Falls back to tracing-only if the platform
/// notification mechanism is unavailable.
pub(super) fn show_notification(title: &str, message: &str) {
    tracing::info!("Notification: {} - {}", title, message);
    #[cfg(target_os = "windows")]
    {
        // Shown under Beamer's own AUMID. This only reads correctly once
        // `ui::windows_shortcut::ensure_shortcut` has registered
        // `crate::WINDOWS_APP_USER_MODEL_ID` with a Start Menu shortcut; see
        // that module for why an unpackaged app needs one at all. There is
        // no PowerShell fallback: an unregistered AUMID makes `show()`
        // return `Ok` while Windows silently drops the toast, so there is no
        // error here to fall back from, and a copy notifying under another
        // program's name is worse than one that stays quiet.
        if let Err(e) = tauri_winrt_notification::Toast::new(crate::WINDOWS_APP_USER_MODEL_ID)
            .title(title)
            .text1(message)
            .show()
        {
            tracing::warn!("Failed to show notification: {}", e);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Err(e) = notify_rust::Notification::new()
            .appname("Beamer")
            .summary(title)
            .body(message)
            .show()
        {
            tracing::warn!("Failed to show notification: {}", e);
        }
    }
}
