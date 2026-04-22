mod audio;
mod config;
mod hotkey;
mod injection;
#[cfg(not(target_os = "windows"))]
mod install;
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

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = install_linux_desktop_entry() {
            tracing::warn!("Failed to install Linux desktop entry: {}", e);
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
            "[Desktop Entry]\nType=Application\nName=Beamer\nIcon=beamer\nExec={}\nStartupWMClass=beamer\nX-GNOME-Autostart-enabled=true\n",
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

// ─── Linux desktop integration ────────────────────────────────────────────────

/// GNOME's dock/taskbar locates app icons by matching a window's Wayland `app_id`
/// (or X11 `WM_CLASS`) against `StartupWMClass` in an installed `.desktop` file —
/// window-level icon hints are ignored under Wayland. Without this install step,
/// Beamer shows up as a generic window in the dock even though the tray icon
/// (which uses AppIndicator and embeds bytes directly) works fine.
///
/// Installs:
///   ~/.local/share/icons/hicolor/512x512/apps/beamer.png
///   ~/.local/share/applications/beamer.desktop   (StartupWMClass=beamer)
///
/// GTK defaults the Wayland app_id to the binary basename when no GApplication
/// id is set (which is Dioxus/tao's behavior), so `beamer` matches.
#[cfg(target_os = "linux")]
fn install_linux_desktop_entry() -> Result<()> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;

    let icon_dir = data_dir.join("icons/hicolor/512x512/apps");
    let icon_path = icon_dir.join("beamer.png");
    std::fs::create_dir_all(&icon_dir)?;

    let icon_bytes: &[u8] = include_bytes!("../assets/icon.png");
    let needs_write = std::fs::metadata(&icon_path)
        .map(|m| m.len() as usize != icon_bytes.len())
        .unwrap_or(true);
    if needs_write {
        std::fs::write(&icon_path, icon_bytes)?;
        tracing::info!("Installed app icon to {:?}", icon_path);
    }

    let apps_dir = data_dir.join("applications");
    std::fs::create_dir_all(&apps_dir)?;
    let desktop_path = apps_dir.join("beamer.desktop");

    let exe_path = std::env::current_exe()?;
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Beamer\nComment=Dictation with cloud transcription\nExec={}\nIcon=beamer\nStartupWMClass=beamer\nTerminal=false\nCategories=Utility;AudioVideo;\n",
        exe_path.display()
    );

    let existing = std::fs::read_to_string(&desktop_path).ok();
    if existing.as_deref() != Some(desktop.as_str()) {
        std::fs::write(&desktop_path, &desktop)?;
        tracing::info!("Installed desktop entry to {:?}", desktop_path);
    }

    Ok(())
}
