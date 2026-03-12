mod audio;
mod config;
mod hotkey;
mod injection;
mod media;
mod orchestrator;
mod sounds;
mod tray;
mod transcription;
mod ui;

use anyhow::Result;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("beamer=info")),
        )
        .init();

    tracing::info!("Beamer starting...");

    if !ensure_single_instance() {
        tracing::warn!("Another instance of Beamer is already running");
        return;
    }

    let config = config::Config::load().unwrap_or_default();
    tracing::info!("Config loaded from {:?}", config::Config::config_path());

    if config.appearance.auto_start {
        if let Err(e) = set_auto_start(true) {
            tracing::warn!("Failed to set auto-start: {}", e);
        }
    }

    // Dioxus owns the main thread and tokio runtime — nothing runs after this
    ui::launch_app();
}

/// Prevent multiple Beamer instances via a named kernel mutex.
/// Returns false if another instance already holds the mutex.
fn ensure_single_instance() -> bool {
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    unsafe {
        let result = CreateMutexW(None, true, w!("Beamer_SingleInstance"));
        match result {
            Ok(_) => {
                let last_error = windows::Win32::Foundation::GetLastError();
                last_error != windows::Win32::Foundation::ERROR_ALREADY_EXISTS
            }
            Err(_) => false,
        }
    }
}

/// Add or remove Beamer from the Windows Run registry key (HKCU\...\Run).
pub fn set_auto_start(enable: bool) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let exe_path = std::env::current_exe()?;
    let exe_str = exe_path.to_string_lossy();

    if enable {
        Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Beamer",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", exe_str),
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
    } else {
        Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "Beamer",
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
    }

    Ok(())
}
