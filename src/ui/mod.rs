pub mod app;
mod app_setup;
pub mod components;
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
pub mod sticky_blocks;
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

    #[cfg(not(target_os = "windows"))]
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

    #[test]
    fn a_target_containing_an_ampersand_encodes_unmodified() {
        // Encoding step only: `&` gets no special treatment. "Never reaches a
        // shell parser" is structural (ShellExecuteW, not cmd), not unit-testable here.
        let target = "https://example.com/?a=1&b=2";
        let wide = to_wide_null(target);
        let decoded: Vec<u16> = target.encode_utf16().collect();
        assert_eq!(&wide[..wide.len() - 1], decoded.as_slice());
    }

    #[test]
    fn empty_string_still_gets_a_terminator() {
        assert_eq!(to_wide_null(""), vec![0]);
    }
}

/// WebView user-data dir. `data_local_dir`, not `data_dir`: on Windows the latter
/// is Roaming `%APPDATA%`, where WebView2's open file handles would collide with
/// resets/cleanups and roam a browser cache at logon. No-op elsewhere (same dir).
/// Existing Windows installs get a fresh profile once: one slower first launch.
pub fn webview_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Beamer")
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
                        .with_inner_size(dioxus::desktop::LogicalSize::new(500.0_f64, 600.0_f64))
                        .with_min_inner_size(dioxus::desktop::LogicalSize::new(500.0_f64, 400.0_f64)),
                )
                .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                .with_exits_when_last_window_closes(false),
        )
        .launch(app::App);
}
