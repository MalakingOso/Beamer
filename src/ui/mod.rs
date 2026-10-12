//! The Dioxus desktop UI. `main.rs` calls `launch_app`, which blocks the main
//! thread in Dioxus's event loop (owner of the process's only tokio runtime)
//! with `app::App` as the root component. Windows: main (home/settings),
//! splash, recording pill (Windows/macOS), and one per open sticky note.
//! Also holds helpers every window shares (`open_external`, `webview_data_dir`).
//! See `agent_docs/dioxus_architecture.md`.

pub mod app;
mod app_menu;
mod app_pill;
mod app_setup;
mod app_splash;
pub mod components;
pub mod done_page;
pub mod fonts;
pub mod history;
pub mod history_page;
pub mod home;
pub mod icons;
mod linux_integration;
pub mod note_layout;
pub mod notes_page;
pub mod pill;
pub mod settings;
pub mod shell_indicator;
pub mod shell_window;
pub mod splash;
pub mod sticky;
pub mod sticky_chips;
pub mod sticky_css;
pub mod sticky_footer;
pub mod sticky_windows;
pub mod work_area;
pub mod status_log;
pub mod tasks_page;
pub mod vocab_page;
mod windows_shortcut;

use std::path::PathBuf;

use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;

/// Null-terminated UTF-16 for Win32 wide-string APIs (`PCWSTR`).
/// An embedded NUL truncates silently at the first zero code unit, as with any
/// Win32 wide-string API; note content is not expected to carry one.
#[cfg(any(target_os = "windows", test))]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Hand a URL or file path to the desktop's registered handler, detached.
/// A failure to open only logs. Not `webbrowser::open`: this also opens local
/// `.ics` files, which are a file association rather than a browser concern.
pub fn open_external(target: &str) {
    #[cfg(target_os = "windows")]
    {
        // ShellExecuteW, not `cmd /C start`: cmd's parser splits on `&`, and
        // `target` comes from note content — a command-injection surface.
        // Plain OS thread, not `spawn_blocking`: event handlers guarantee no
        // entered tokio context.
        let owned = target.to_owned();
        std::thread::spawn(move || unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

            // Shell APIs expect an STA; `RPC_E_CHANGED_MODE` means no reference
            // was taken, same balance as `injection::uia`.
            let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let we_initialized = hr.is_ok();

            let wide_target = to_wide_null(&owned);
            let outcome = ShellExecuteW(
                HWND::default(),
                PCWSTR::null(),
                PCWSTR::from_raw(wide_target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            // Pseudo-handle, not a real HINSTANCE: > 32 is success, else SE_ERR_*.
            if outcome.0 as usize <= 32 {
                tracing::warn!("Could not open {:?}: ShellExecuteW returned {}", owned, outcome.0 as usize);
            }

            if we_initialized {
                CoUninitialize();
            }
        });
    }

    #[cfg(target_os = "linux")]
    {
        let result = std::process::Command::new("xdg-open").arg(target).spawn();
        if let Err(e) = result {
            tracing::warn!("Could not open {:?}: {}", target, e);
        }
    }
}

#[cfg(test)]
mod open_external_tests {
    use super::to_wide_null;

    #[test]
    fn appends_exactly_one_null_terminator() {
        let wide = to_wide_null("hello");
        assert_eq!(wide.last(), Some(&0));
        assert_eq!(wide.len(), "hello".encode_utf16().count() + 1);
    }
}

#[cfg(test)]
mod main_window_tests {
    use super::main_window_restored_size;

    #[test]
    fn a_collapsed_window_restores_to_the_design_size() {
        // The live failure: 158x26 physical px at 150% scaling is ~105x17
        // logical px, far below the 500x400 floor.
        assert_eq!(main_window_restored_size(105.0, 17.0), Some((500.0, 600.0)));
        // Below the floor in only one dimension still restores.
        assert_eq!(main_window_restored_size(800.0, 17.0), Some((500.0, 600.0)));
    }

    #[test]
    fn a_window_at_or_above_the_floor_is_left_alone() {
        assert_eq!(main_window_restored_size(500.0, 400.0), None);
        assert_eq!(main_window_restored_size(800.0, 700.0), None);
    }
}

/// WebView user-data dir. `data_local_dir`, not `data_dir`: on Windows the latter
/// is Roaming `%APPDATA%`, where WebView2's open file handles would collide with
/// resets/cleanups and roam a browser cache at logon. No-op elsewhere (same dir).
pub fn webview_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Beamer")
}

/// Main window's design size and floor (logical px). The floor is also the
/// builder's `with_min_inner_size`, but that is not reliably enforced for an
/// undecorated transparent window on Windows: a running Beamer was found with
/// its main window at 158x26 physical px, showing only the titlebar as a blank
/// pill. [`ensure_main_window_size`] therefore re-asserts it at runtime.
pub(super) const MAIN_WINDOW_W: f64 = 500.0;
pub(super) const MAIN_WINDOW_H: f64 = 600.0;
pub(super) const MAIN_WINDOW_MIN_W: f64 = 500.0;
pub(super) const MAIN_WINDOW_MIN_H: f64 = 400.0;

/// The size to restore a below-floor main window to, or `None` to leave it
/// alone (a window the user enlarged on purpose stays enlarged).
pub(super) fn main_window_restored_size(logical_w: f64, logical_h: f64) -> Option<(f64, f64)> {
    if logical_w < MAIN_WINDOW_MIN_W || logical_h < MAIN_WINDOW_MIN_H {
        Some((MAIN_WINDOW_W, MAIN_WINDOW_H))
    } else {
        None
    }
}

/// Grow the main window back to its design size if it is below the floor.
/// Called at startup and on every tray/menu show, the two moments a collapsed
/// window would otherwise stay collapsed forever (nothing else resizes it).
pub(super) fn ensure_main_window_size(window: &dioxus::desktop::DesktopContext) {
    let scale = window.scale_factor();
    if scale <= 0.0 {
        return;
    }
    let size = window.inner_size();
    if let Some((w, h)) =
        main_window_restored_size(size.width as f64 / scale, size.height as f64 / scale)
    {
        tracing::warn!(
            "Main window is {}x{} logical px, below its floor; restoring to {w}x{h}",
            size.width as f64 / scale,
            size.height as f64 / scale
        );
        window.set_inner_size(dioxus::desktop::LogicalSize::new(w, h));
    }
}

/// Launch the Dioxus desktop window. Blocks the main thread; the tray icon keeps
/// the process alive while the window is hidden (`WindowHides`).
pub fn launch_app() {
    let icon_bytes = crate::assets::ICON_PNG;
    let icon_image = image::load_from_memory(icon_bytes)
        .expect("Failed to load icon.png")
        .to_rgba8();
    let (w, h) = icon_image.dimensions();
    let window_icon =
        Icon::from_rgba(icon_image.into_raw(), w, h).expect("Failed to create window icon");

    LaunchBuilder::new()
        .with_cfg(
            Config::new()
                .with_data_directory(webview_data_dir())
                .with_background_color((0, 0, 0, 0))
                .with_window(
                    WindowBuilder::new()
                        .with_title("Beamer")
                        // Start hidden: the splash owns the launch moment (non-Windows
                        // reveals the main window when the splash dismisses).
                        .with_visible(false)
                        .with_decorations(false)
                        .with_transparent(true)
                        .with_window_icon(Some(window_icon))
                        .with_inner_size(dioxus::desktop::LogicalSize::new(MAIN_WINDOW_W, MAIN_WINDOW_H))
                        .with_min_inner_size(dioxus::desktop::LogicalSize::new(MAIN_WINDOW_MIN_W, MAIN_WINDOW_MIN_H)),
                )
                // Inline, not a manganis `asset!()` link: a hashed asset file
                // lives beside the exe, so a self-update that swaps only the
                // binary requests a filename the old install never had and the
                // window renders unstyled. `tests/packaging.rs` forbids `asset!(`.
                .with_custom_head(format!(
                    "<style>{}{}</style>",
                    fonts::embedded_font_css(),
                    include_str!("../../assets/styles.css")
                ))
                .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                .with_exits_when_last_window_closes(false),
        )
        .launch(app::App);
}
