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
pub mod sticky_windows;
pub mod status_log;
pub mod vocab_page;

use std::path::PathBuf;

use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;

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
