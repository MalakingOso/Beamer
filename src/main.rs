mod assets;
mod audio;
mod config;
mod hotkey;
mod injection;
#[cfg(not(target_os = "windows"))]
mod install;
mod llm;
mod media;
mod notes;
mod orchestrator;
mod sounds;
mod tray;
mod transcription;
mod ui;
mod update;
mod warmup;

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

    // Bakes `connect_timeout_ms` into the shared LLM client's `OnceLock` for
    // the rest of the process. Must happen before anything reaches
    // `llm::client::http_client()`, including settings::LocalAiCard's own
    // startup probe, so this runs as early as the config is available, ahead
    // of `ui::launch_app()`. See `llm::client::init_http_client` for why the
    // value can't just be read per-request instead.
    llm::client::init_http_client(std::time::Duration::from_millis(config.llm.connect_timeout_ms));

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
        silence_ayatana_deprecation_warning();
    }

    // Dioxus owns the main thread and tokio runtime — nothing runs after this
    ui::launch_app();
}

// ─── Single-instance guard ────────────────────────────────────────────────────

/// Prevent multiple Beamer instances. Returns false if another instance is already running.
///
/// The guard this takes is process-wide and must be handed off explicitly
/// before spawning a successor — see [`release_single_instance`].
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

/// Give up this process's claim on the single-instance guard.
///
/// Must be called before spawning a replacement Beamer that will immediately
/// run `ensure_single_instance()` itself — otherwise the successor sees *this*
/// process still holding the guard and exits, which is what used to make
/// "Restart Now" after an update silently kill the app instead of relaunching
/// it (`update::restart_app` spawns the child while the parent is still
/// alive). Also called on the tray Quit path so a stale lockfile never
/// outlives the process.
///
/// Idempotent: calling it twice, or without ever having acquired the guard, is
/// a no-op.
pub fn release_single_instance() {
    #[cfg(target_os = "windows")]
    {
        release_single_instance_windows();
    }
    #[cfg(not(target_os = "windows"))]
    {
        release_single_instance_lockfile();
    }
}

/// Raw `HANDLE` value for the single-instance mutex, or 0 when not held.
/// Stored as a plain integer because `HANDLE` is a raw-pointer newtype and so
/// isn't `Sync`; the value is only ever produced by `CreateMutexW` on the main
/// thread and consumed by `CloseHandle`, so round-tripping it through an
/// atomic is sound.
#[cfg(target_os = "windows")]
static INSTANCE_MUTEX: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(target_os = "windows")]
fn ensure_single_instance_windows() -> bool {
    use std::sync::atomic::Ordering;
    use windows::core::w;
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        let result = CreateMutexW(None, true, w!("Beamer_SingleInstance"));
        match result {
            Ok(handle) => {
                // GetLastError must be read before anything else can clobber it.
                let last_error = windows::Win32::Foundation::GetLastError();
                if last_error == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
                    // Someone else owns it; close our (non-owning) handle so we
                    // don't leak it, and refuse to start.
                    let _ = windows::Win32::Foundation::CloseHandle(handle);
                    false
                } else {
                    INSTANCE_MUTEX.store(handle.0 as isize, Ordering::SeqCst);
                    true
                }
            }
            Err(_) => false,
        }
    }
}

#[cfg(target_os = "windows")]
fn release_single_instance_windows() {
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::ReleaseMutex;

    let raw = INSTANCE_MUTEX.swap(0, Ordering::SeqCst);
    if raw == 0 {
        return;
    }
    let handle = HANDLE(raw as *mut std::ffi::c_void);
    unsafe {
        // Release ownership first (we passed bInitialOwner = true), then close.
        let _ = ReleaseMutex(handle);
        let _ = CloseHandle(handle);
    }
    tracing::info!("Released single-instance mutex");
}

/// Path of the lockfile this process owns, or `None` when the guard isn't held.
#[cfg(not(target_os = "windows"))]
static INSTANCE_LOCKFILE: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

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
    if std::fs::write(&lock_path, format!("{}", std::process::id())).is_ok() {
        *INSTANCE_LOCKFILE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(lock_path);
    }
    true
}

#[cfg(not(target_os = "windows"))]
fn release_single_instance_lockfile() {
    let taken = INSTANCE_LOCKFILE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(path) = taken {
        // Only remove it if it's still ours — a successor that already
        // acquired the guard must not have its lockfile deleted out from
        // under it.
        let still_ours = std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| c.trim().parse::<u32>().ok())
            .map(|pid| pid == std::process::id())
            .unwrap_or(false);
        if still_ours {
            let _ = std::fs::remove_file(&path);
            tracing::info!("Released single-instance lockfile at {:?}", path);
        }
    }
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

/// libayatana-appindicator 0.5.93+ emits a deprecation g_warning() once when the
/// first indicator is created. tray-icon still binds the deprecated library (the
/// -glib successor has an incompatible API), so the notice is unavoidable noise.
/// Install a GLib log handler for that domain that drops the deprecation notice
/// and forwards everything else to the default handler. Uses raw FFI because
/// libglib-2.0 is already linked via gtk; depending on the glib crate would pin
/// us to whatever gtk-rs version dioxus' tree happens to use.
#[cfg(target_os = "linux")]
fn silence_ayatana_deprecation_warning() {
    use std::os::raw::{c_char, c_int, c_uint, c_void};

    type GLogFunc =
        extern "C" fn(*const c_char, c_int, *const c_char, *mut c_void);

    extern "C" {
        fn g_log_set_handler(
            log_domain: *const c_char,
            log_levels: c_int,
            log_func: GLogFunc,
            user_data: *mut c_void,
        ) -> c_uint;
        fn g_log_default_handler(
            log_domain: *const c_char,
            log_level: c_int,
            message: *const c_char,
            unused_data: *mut c_void,
        );
    }

    const G_LOG_FLAG_RECURSION: c_int = 1 << 0;
    const G_LOG_FLAG_FATAL: c_int = 1 << 1;
    const G_LOG_LEVEL_WARNING: c_int = 1 << 4;

    extern "C" fn drop_deprecation_notice(
        domain: *const c_char,
        level: c_int,
        message: *const c_char,
        data: *mut c_void,
    ) {
        let is_deprecation = !message.is_null()
            && unsafe { std::ffi::CStr::from_ptr(message) }
                .to_bytes()
                .windows(b"deprecated".len())
                .any(|w| w == b"deprecated");
        if !is_deprecation {
            unsafe { g_log_default_handler(domain, level, message, data) };
        }
    }

    unsafe {
        g_log_set_handler(
            c"libayatana-appindicator".as_ptr(),
            G_LOG_LEVEL_WARNING | G_LOG_FLAG_FATAL | G_LOG_FLAG_RECURSION,
            drop_deprecation_notice,
            std::ptr::null_mut(),
        );
    }
}

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

    let icon_bytes: &[u8] = assets::ICON_PNG;
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
