//! Cold-start warmup splash hook for `App`. Runs once from `App()`'s render body.

use dioxus::desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
#[cfg(target_os = "windows")]
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
use dioxus::desktop::{Config as DesktopConfig, DesktopContext, WindowBuilder};
use dioxus::prelude::*;

use crate::ui::splash::{SplashWindow, SPLASH_CSS};
use crate::warmup::{self, WarmupProgress};

/// Cold-start warmup splash: pays the one-time costs that would otherwise stall
/// the first recording, then closes itself and flips `app_ready`.
pub(super) fn setup_splash(window: DesktopContext, mut app_ready: Signal<bool>) {
    let warmup_progress = use_signal(WarmupProgress::default);
    let mut splash_ctx: Signal<Option<DesktopContext>> = use_signal(|| None);

    use_hook({
        let window = window.clone();
        move || {
            spawn(async move {
                let scale = window
                    .primary_monitor()
                    .map(|m| m.scale_factor())
                    .unwrap_or(1.0);
                let monitor_size = window
                    .primary_monitor()
                    .map(|m| m.size())
                    .unwrap_or(PhysicalSize::new(1920, 1080));

                let splash_w = (280.0 * scale) as u32;
                let splash_h = (280.0 * scale) as u32;
                let x = (monitor_size.width.saturating_sub(splash_w)) / 2;
                let y = (monitor_size.height.saturating_sub(splash_h)) / 2;

                #[allow(unused_mut)]
                let mut builder = WindowBuilder::new()
                    .with_title("Beamer")
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_transparent(true)
                    .with_always_on_top(true)
                    .with_focusable(false)
                    .with_inner_size(PhysicalSize::new(splash_w, splash_h))
                    .with_position(PhysicalPosition::new(x as i32, y as i32));
                #[cfg(target_os = "windows")]
                let builder = builder
                    .with_skip_taskbar(true)
                    .with_undecorated_shadow(false);

                let cfg = DesktopConfig::new()
                    .with_data_directory(super::webview_data_dir())
                    .with_window(builder)
                    .with_background_color((0, 0, 0, 0))
                    .with_custom_head(format!("<style>{}</style>", SPLASH_CSS))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(SplashWindow);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;
                splash_ctx.set(Some(ctx));

                let started = std::time::Instant::now();
                warmup::warm_all(warmup_progress).await;

                // Floor matches the splash bar's 1500ms CSS keyframe (see SPLASH_CSS).
                let min_visible = std::time::Duration::from_millis(1500);
                let elapsed = started.elapsed();
                if elapsed < min_visible {
                    tokio::time::sleep(min_visible - elapsed).await;
                }

                if let Some(ctx) = splash_ctx.read().as_ref() {
                    ctx.close();
                }
                splash_ctx.set(None);

                app_ready.set(true);

                // Non-Windows reveals the main window here; Windows waits for tray-click.
                #[cfg(not(target_os = "windows"))]
                window.set_visible(true);
            });
        }
    });
}
