pub mod app;
pub mod components;
pub mod glow;
pub mod history;
pub mod history_page;
pub mod home;
pub mod icons;
pub mod overlay;
pub mod settings;
pub mod status_log;

use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;

pub fn launch_app() {
    LaunchBuilder::new()
        .with_cfg(
            Config::new()
                .with_window(
                    WindowBuilder::new()
                        .with_title("Beamer")
                        .with_visible(false)
                        .with_decorations(false)
                        .with_inner_size(dioxus::desktop::LogicalSize::new(500.0_f64, 600.0_f64)),
                )
                .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                .with_exits_when_last_window_closes(false),
        )
        .launch(app::App);
}
