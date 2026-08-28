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
        // Real branding first. This only reads correctly if the Start Menu
        // shortcut's AUMID matches `crate::WINDOWS_APP_USER_MODEL_ID` (set on
        // the process in `main`) and `identifier` in `Dioxus.toml`. Three
        // places have to agree, none of which dioxus checks for us, and
        // dioxus's `NsisSettings` has no field to set the shortcut's AUMID
        // from here. A portable (non-installed) run has no such shortcut, so
        // the AUMID is unregistered there and Windows can suppress the toast
        // outright instead of just mislabeling it. That is what the fallback
        // below is for: a copy that was never installed still notifies, just
        // under PowerShell's name.
        let primary = winrt_notification::Toast::new(crate::WINDOWS_APP_USER_MODEL_ID)
            .title(title)
            .text1(message)
            .show();

        if let Err(primary_err) = primary {
            tracing::warn!(
                "Notification under Beamer's app id failed ({}), falling back to PowerShell's",
                primary_err
            );
            if let Err(e) = winrt_notification::Toast::new(winrt_notification::Toast::POWERSHELL_APP_ID)
                .title(title)
                .text1(message)
                .show()
            {
                tracing::warn!("Failed to show notification: {}", e);
            }
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
