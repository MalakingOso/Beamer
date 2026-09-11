// No console window in release builds; also required by `dx bundle`
// (`/SUBSYSTEM:WINDOWS` links `WinMain`, not `main`).
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

mod assets;
mod audio;
mod config;
mod hotkey;
mod injection;
#[cfg(not(target_os = "windows"))]
mod install;
mod llm;
mod media;
mod model_setup;
mod notes;
mod orchestrator;
mod sounds;
mod tray;
mod transcription;
mod ui;
mod update;
mod warmup;

use anyhow::Result;

/// Windows AppUserModelID: what toasts and the taskbar display as "Beamer".
/// Must match `identifier` in `Dioxus.toml`, the shortcut property written by
/// `ui::windows_shortcut::ensure_shortcut`, and the `Toast::new` app id.
/// Nothing checks this; a mismatch fails silently (toast never appears).
#[cfg(target_os = "windows")]
pub(crate) const WINDOWS_APP_USER_MODEL_ID: &str = "com.beamer.app";

/// Our identity to the desktop: the Wayland `app_id` (pinned by
/// [`set_gtk_prgname`]), the basename of both `.desktop` files, their
/// `StartupWMClass`, and the icon name they reference.
///
/// Every one of those has to be the same string or Mutter cannot attach an
/// icon to our windows, and the mismatch fails silently: the window just
/// gets the generic placeholder. The Windows counterpart is
/// [`WINDOWS_APP_USER_MODEL_ID`].
#[cfg(not(target_os = "windows"))]
pub(crate) const APP_ID: &str = "beamer";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("beamer=info")),
        )
        .init();

    tracing::info!("Beamer starting...");

    // Before any window/toast exists so all are attributed to Beamer.
    #[cfg(target_os = "windows")]
    set_windows_app_user_model_id();

    if !ensure_single_instance() {
        tracing::warn!("Another instance of Beamer is already running");
        return;
    }

    let config = config::Config::load().unwrap_or_default();
    tracing::info!("Config loaded from {:?}", config::Config::config_path());

    // Must precede any `llm::client::http_client()` use (including the
    // settings probe). `connect_timeout()` clamps to the minimum so a
    // hand-edited `0` can't build a client that fails every connection.
    llm::client::init_http_client(config.llm.connect_timeout());

    if config.appearance.auto_start {
        if let Err(e) = set_auto_start(true) {
            tracing::warn!("Failed to set auto-start: {}", e);
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Must precede `ui::launch_app()`, which initializes GTK.
        set_gtk_prgname();
        if let Err(e) = install_linux_desktop_entry() {
            tracing::warn!("Failed to install Linux desktop entry: {}", e);
        }
        silence_ayatana_deprecation_warning();
    }

    // Dioxus owns the main thread and tokio runtime past this point.
    ui::launch_app();
}

/// Register the process under [`WINDOWS_APP_USER_MODEL_ID`].
#[cfg(target_os = "windows")]
fn set_windows_app_user_model_id() {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let aumid = HSTRING::from(WINDOWS_APP_USER_MODEL_ID);
    // SAFETY: stores the string for the process; `HSTRING` guarantees validity.
    let result = unsafe { SetCurrentProcessExplicitAppUserModelID(&aumid) };
    if let Err(e) = result {
        tracing::debug!("Failed to set process AppUserModelID: {}", e);
    }
}

// Single-instance guard.

/// Prevent multiple instances. Returns false if another is already running.
/// Release via [`release_single_instance`] before spawning a successor.
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

/// Give up the single-instance guard. Must precede spawning a successor (see
/// `update::restart_app`); also called on Quit so no stale lockfile survives.
/// Idempotent.
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

/// Single-instance mutex as a raw integer (`HANDLE` isn't `Sync`; produced by
/// `CreateMutexW` on main, consumed by `CloseHandle`). 0 when not held.
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
                // Read before anything clobbers it.
                let last_error = windows::Win32::Foundation::GetLastError();
                if last_error == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
                    // Rival instance owns it; close our handle, refuse to start.
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
        // Release ownership (we passed bInitialOwner = true), then close.
        let _ = ReleaseMutex(handle);
        let _ = CloseHandle(handle);
    }
    tracing::info!("Released single-instance mutex");
}

/// Lockfile this process owns, if the guard is held.
#[cfg(not(target_os = "windows"))]
static INSTANCE_LOCKFILE: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

#[cfg(not(target_os = "windows"))]
fn ensure_single_instance_lockfile() -> bool {
    let lock_path = std::env::temp_dir().join("beamer.lock");

    // A stale PID file (dead process) doesn't count; `/proc/<pid>` probes liveness.
    if let Ok(contents) = std::fs::read_to_string(&lock_path) {
        if let Ok(pid) = contents.trim().parse::<u32>() {
            if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                return false;
            }
        }
    }

    // Best-effort: never fail startup over the lockfile.
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
        // Only remove if still ours — a successor may already hold the guard.
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

// Auto-start.

/// Add or remove Beamer from system auto-start.
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

/// True when the running binary sits inside a cargo/`dx` build directory
/// rather than an installed location.
///
/// Such a path is not a durable target for a `.desktop` `Exec=`: `dx` renames
/// dev binaries per build (`beamer-f9c230e6`), and `cargo clean` deletes the
/// tree outright. Writing one into the user's data dir shadows the packaged
/// entry (user data dir wins in XDG precedence) and pins the dock to a binary
/// that will not exist tomorrow.
#[cfg(not(target_os = "windows"))]
fn is_dev_build_exe(exe: &std::path::Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
}

/// The installed `beamer` on `PATH`, if there is one.
#[cfg(not(target_os = "windows"))]
fn packaged_beamer_on_path() -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(APP_ID))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// What an autostart entry should launch.
///
/// A dev build prefers the packaged binary: the user asked for autostart, and
/// honoring that with a path that survives the next rebuild is more useful
/// than honoring it literally. With no packaged binary the dev path is still
/// written, because a working-today autostart beats none at all.
#[cfg(not(target_os = "windows"))]
fn autostart_exec_path() -> Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    if !is_dev_build_exe(&exe) {
        return Ok(exe);
    }
    match packaged_beamer_on_path() {
        Some(packaged) => {
            tracing::info!("Dev build: autostart points at the installed {:?}", packaged);
            Ok(packaged)
        }
        None => {
            tracing::warn!(
                "Dev build and no installed beamer on PATH: autostart pinned to {:?}, \
                 which the next rebuild will invalidate",
                exe
            );
            Ok(exe)
        }
    }
}

/// XDG autostart entry (`~/.config/autostart/beamer.desktop`).
#[cfg(not(target_os = "windows"))]
fn set_auto_start_xdg(enable: bool) -> Result<()> {
    let autostart_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
        .join("autostart");

    let desktop_path = autostart_dir.join(format!("{APP_ID}.desktop"));

    if enable {
        std::fs::create_dir_all(&autostart_dir)?;
        let exe_path = autostart_exec_path()?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Beamer\nIcon={APP_ID}\nExec={}\nStartupWMClass={APP_ID}\nX-GNOME-Autostart-enabled=true\n",
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

// Linux desktop integration.

/// Pin GTK's program name to [`APP_ID`], which is what GTK3 hands
/// Wayland as the toplevel `app_id`.
///
/// Without this the `app_id` is whatever `argv[0]`'s basename happens to be.
/// That is `beamer` for a packaged install and matches, but `dx` names its
/// dev binaries with a build hash (`beamer-f9c230e6`), so every window from a
/// `dx build`/`dx serve` run announced an `app_id` no `.desktop` file
/// declares, and GNOME fell back to the generic placeholder icon.
///
/// Safe to call before GTK: `gdk_parse_args` only derives a prgname from
/// `argv[0]` when one is not already set, so setting it first wins. Raw FFI
/// for the same reason as [`silence_ayatana_deprecation_warning`] below, to
/// avoid pinning a glib crate version.
#[cfg(target_os = "linux")]
fn set_gtk_prgname() {
    use std::os::raw::c_char;

    extern "C" {
        fn g_set_prgname(prgname: *const c_char);
    }

    let name = std::ffi::CString::new(APP_ID).expect("APP_ID has no interior nul");
    unsafe { g_set_prgname(name.as_ptr()) };
    tracing::debug!("GTK prgname pinned to {:?}", APP_ID);
}

/// Drop libayatana-appindicator's one-time deprecation warning; forward the rest
/// to the default handler. Raw FFI avoids pinning a glib crate version.
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

/// Install the icon + `.desktop` file so GNOME's dock matches our windows.
///
/// Wayland ignores window-level icon hints; Mutter matches the toplevel
/// `app_id` against `StartupWMClass` (or the `.desktop` basename). Our
/// `app_id` is [`APP_ID`], pinned by [`set_gtk_prgname`] rather than
/// left to the binary's filename, so `beamer` matches either way.
///
/// Skipped entirely for a dev build. This entry is the app's own
/// housekeeping, not something the user asked for, and one written from a
/// `target/` path shadows the packaged entry for good (the user data dir
/// wins in XDG precedence) while pointing at a binary `dx` will rename on
/// the next build. Autostart is handled differently, see
/// [`autostart_exec_path`]: that one *is* a user gesture.
#[cfg(target_os = "linux")]
fn install_linux_desktop_entry() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    if is_dev_build_exe(&exe_path) {
        tracing::info!(
            "Dev build at {:?}: leaving the packaged desktop entry alone",
            exe_path
        );
        return Ok(());
    }

    let data_dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?;

    let icon_dir = data_dir.join("icons/hicolor/512x512/apps");
    let icon_path = icon_dir.join(format!("{APP_ID}.png"));
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
    let desktop_path = apps_dir.join(format!("{APP_ID}.desktop"));

    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Beamer\nComment=Dictation with cloud transcription\nExec={}\nIcon={APP_ID}\nStartupWMClass={APP_ID}\nTerminal=false\nCategories=Utility;AudioVideo;\n",
        exe_path.display()
    );

    let existing = std::fs::read_to_string(&desktop_path).ok();
    if existing.as_deref() != Some(desktop.as_str()) {
        std::fs::write(&desktop_path, &desktop)?;
        tracing::info!("Installed desktop entry to {:?}", desktop_path);
    }

    Ok(())
}
