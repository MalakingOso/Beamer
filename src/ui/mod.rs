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

/// Encode `s` as a null-terminated UTF-16 buffer, the form Win32 wide-string
/// APIs (`PCWSTR`) need.
///
/// Split out so it can be unit-tested without a Windows target: it is plain
/// `char` encoding with no OS call in it, so the terminator-appending logic
/// is checkable on any host. Gated on `test` as well as `windows`, since its
/// only non-test caller is windows-only and a plain Linux `cargo check` has
/// no test harness to keep it alive otherwise.
///
/// Does not guard against an embedded NUL in `s`: if one is present,
/// `PCWSTR` (which reads up to the first zero code unit) truncates there
/// silently, same as any Win32 wide-string API. A truncated target can only
/// fail to open or open a shorter path; it cannot make `ShellExecuteW` run
/// something else, so this is an ordinary correctness limitation rather than
/// a reopening of the injection risk `open_external` exists to close. Note
/// content is not expected to carry a NUL.
#[cfg(any(target_os = "windows", test))]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Hand a URL or a file path to whatever the desktop has registered for it.
///
/// Detached either way, but the two platforms detach differently. On Linux
/// this is `Command::spawn`: the child process is left to run and its exit
/// status is never collected, because the opener keeps running for as long
/// as the browser or calendar does, and a failure to spawn is logged and
/// nothing else. On Windows there is no child process to leave running:
/// `ShellExecuteW` runs on a detached OS thread so the caller isn't blocked
/// on it, but that thread does check the call's own outcome (the `<= 32`
/// pseudo-handle test below) and logs a failure there, which the Linux path
/// has no equivalent of. Either way, a failure to open does nothing more
/// than log: the user clicked a link, and freezing the note over it would be
/// worse than the link not opening.
///
/// Not `webbrowser::open`, even though dioxus-desktop already depends on it:
/// this also has to open a local `.ics`, which is a file association rather
/// than a browser concern.
pub fn open_external(target: &str) {
    #[cfg(target_os = "windows")]
    {
        // ShellExecuteW, not `cmd /C start "" <target>`. The old form put the
        // target through cmd's own parser, which only quotes arguments Rust
        // decided need it (spaces or quotes). An unescaped `&` still splits
        // there, so the browser got a truncated URL and cmd ran whatever came
        // after the `&` as its own command. `target` comes from note content
        // (a link chip, an `.ics` export), so this is a command-injection
        // surface reachable from anything a note captured. ShellExecuteW
        // never touches a shell parser; the whole target travels as one
        // opaque wide string.
        //
        // Runs on a plain OS thread, not `tokio::task::spawn_blocking`: this
        // function is called synchronously from Dioxus event handlers with no
        // guarantee the calling thread has an entered tokio context, and a
        // detached `std::thread::spawn` keeps the same "fire and forget"
        // character as the `Command::spawn` call it replaces, without
        // depending on one.
        let owned = target.to_owned();
        std::thread::spawn(move || unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

            // Shell APIs expect an STA; balance the reference the same way
            // `injection::uia` does, since `RPC_E_CHANGED_MODE` means this
            // thread was already initialized in a different apartment and no
            // reference was actually taken.
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
            // ShellExecuteW's return is a status pseudo-handle, not a real
            // HINSTANCE: values above 32 mean success, anything else is an
            // SE_ERR_* code.
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
        // Pins the UTF-16 encoding step only: `to_wide_null` does not parse,
        // escape, or otherwise treat `&` specially, so a target carrying one
        // comes out as plain code units plus the terminator, same as any
        // other string. This does not, and cannot, test "never reaches a
        // shell parser" - that property comes from calling `ShellExecuteW`
        // instead of `cmd`, which is structural to `open_external` and not
        // unit-testable on a Linux host.
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

/// WebView user-data dir must be writable and persistent across launches.
pub fn webview_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Beamer")
}

/// Configure and launch the Dioxus desktop window. This call blocks the main
/// thread for the lifetime of the application — the tray icon keeps the process
/// alive even when the window is hidden (`WindowCloseBehaviour::WindowHides`).
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
                        // Always start hidden — the splash window owns the
                        // launch moment. After the splash dismisses, app.rs
                        // reveals the main window on platforms where it
                        // would have been visible at launch (everything
                        // except Windows, which waits for tray-click).
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
