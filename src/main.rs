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
mod update;

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

// ─── Single-instance guard ────────────────────────────────────────────────────

/// Prevent multiple Beamer instances. Returns false if another instance is already running.
fn ensure_single_instance() -> bool {
    #[cfg(target_os = "windows")]
    {
        ensure_single_instance_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        ensure_single_instance_lockfile()
    }
}

#[cfg(target_os = "windows")]
fn ensure_single_instance_windows() -> bool {
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

#[cfg(not(target_os = "windows"))]
fn ensure_single_instance_lockfile() -> bool {
    let lock_path = std::env::temp_dir().join("beamer.lock");

    // If a PID file exists, check if that process is still alive
    if let Ok(contents) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = contents.trim().parse::<u32>() {
            // /proc/<pid> exists for every running process on Linux
            if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                return false;
            }
        }
    }

    // Write our PID — best effort, don't fail startup if this doesn't work
    let _ = std::fs::write(&lock_path, format!("{}", std::process::id()));
    true
}

// ─── Auto-start ───────────────────────────────────────────────────────────────

/// Add or remove Beamer from the system's auto-start mechanism.
pub fn set_auto_start(enable: bool) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        set_auto_start_windows(enable)
    }
    #[cfg(not(target_os = "windows"))]
    {
        set_auto_start_xdg(enable)
    }
}

#[cfg(target_os = "windows")]
fn set_auto_start_windows(enable: bool) -> Result<()> {
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

/// XDG autostart: writes/removes ~/.config/autostart/beamer.desktop
#[cfg(not(target_os = "windows"))]
fn set_auto_start_xdg(enable: bool) -> Result<()> {
    let autostart_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
        .join("autostart");

    let desktop_path = autostart_dir.join("beamer.desktop");

    if enable {
        std::fs::create_dir_all(&autostart_dir)?;
        let exe_path = std::env::current_exe()?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Beamer\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            exe_path.display()
        );
        std::fs::write(&desktop_path, desktop)?;
        tracing::info!("XDG autostart written to {:?}", desktop_path);
    } else if desktop_path.exists() {
        std::fs::remove_file(&desktop_path)?;
        tracing::info!("XDG autostart entry removed");
    }

    Ok(())
}
