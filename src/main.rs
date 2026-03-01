#![allow(dead_code)]

mod audio;
mod config;
mod hotkey;
mod injection;
mod orchestrator;
mod tray;
mod transcription;
mod ui;

use anyhow::Result;

fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("beamer=info")),
        )
        .init();

    tracing::info!("Beamer starting...");

    // Single-instance check
    if !ensure_single_instance() {
        tracing::warn!("Another instance of Beamer is already running");
        return;
    }

    // Load config
    let config = config::Config::load().unwrap_or_default();
    tracing::info!("Config loaded from {:?}", config::Config::config_path());

    // Handle auto-start registry
    if config.appearance.auto_start {
        if let Err(e) = set_auto_start(true) {
            tracing::warn!("Failed to set auto-start: {}", e);
        }
    }

    // Launch Dioxus desktop app (blocks main thread)
    ui::launch_app();
}

fn ensure_single_instance() -> bool {
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    unsafe {
        let result = CreateMutexW(None, true, w!("Beamer_SingleInstance"));
        match result {
            Ok(_) => {
                // Check if mutex already existed
                let last_error = windows::Win32::Foundation::GetLastError();
                last_error != windows::Win32::Foundation::ERROR_ALREADY_EXISTS
            }
            Err(_) => false,
        }
    }
}

fn set_auto_start(enable: bool) -> Result<()> {
    use std::process::Command;

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
            .output()?;
    }

    Ok(())
}
