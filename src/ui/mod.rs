pub mod app;
pub mod components;
pub mod history;
pub mod history_page;
pub mod home;
pub mod icons;
pub mod pill;
pub mod settings;
pub mod status_log;

use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;

/// Configure and launch the Dioxus desktop window. This call blocks the main
/// thread for the lifetime of the application — the tray icon keeps the process
/// alive even when the window is hidden (`WindowCloseBehaviour::WindowHides`).
pub fn launch_app() {
    let icon_bytes = include_bytes!("../../assets/icon.png");
    let icon_image = image::load_from_memory(icon_bytes)
        .expect("Failed to load icon.png")
        .to_rgba8();
    let (w, h) = icon_image.dimensions();
    let window_icon =
        Icon::from_rgba(icon_image.into_raw(), w, h).expect("Failed to create window icon");

    LaunchBuilder::new()
        .with_cfg(
            Config::new()
                .with_window(
                    WindowBuilder::new()
                        .with_title("Beamer")
                        .with_visible(false)
                        .with_decorations(false)
                        .with_window_icon(Some(window_icon))
                        .with_inner_size(dioxus::desktop::LogicalSize::new(500.0_f64, 600.0_f64)),
                )
                .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                .with_exits_when_last_window_closes(false),
        )
        .launch(app::App);
}
