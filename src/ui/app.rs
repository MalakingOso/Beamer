use dioxus::desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
use dioxus::desktop::trayicon::{init_tray_icon, MouseButton, MouseButtonState, TrayIconEvent};
use dioxus::desktop::{
    use_tray_icon_event_handler, use_tray_menu_event_handler, use_window,
    Config as DesktopConfig, DesktopContext, HotKeyState, ShortcutHandle, WindowBuilder,
};
use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::orchestrator;
use crate::tray;
use crate::ui::pill::RecordingPill;
use crate::ui::history::TranscriptionHistory;
use crate::ui::history_page::HistoryPage;
use crate::ui::home::HomePage;
use crate::ui::icons::{IconClockCounterClockwise, IconGear, IconHouse, IconMinus, IconX};
use crate::ui::settings::SettingsPage;
use crate::ui::status_log::StatusLog;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    History,
    Settings,
}

#[component]
pub fn App() -> Element {
    let items = use_hook(|| {
        let (menu, items) = tray::build_tray_menu();
        let icon = tray::load_icon();
        init_tray_icon(menu, Some(icon));
        items
    });

    let window = use_window();

    let mut current_page = use_signal(|| Page::Home);
    let is_recording = use_signal(|| false);
    let overlay_text = use_signal(String::new);
    let last_injection = use_signal(|| "No injection yet".to_string());
    let history = use_signal(TranscriptionHistory::load);
    let config = use_signal(|| Config::load().unwrap_or_default());
    let status_log = use_signal(StatusLog::new);

    // Shared signals — consumed by child components
    use_context_provider(|| is_recording);
    use_context_provider(|| overlay_text);
    use_context_provider(|| last_injection);
    use_context_provider(|| config);

    // Recording pill window — small, transparent, click-through, always-on-top
    let mut pill_ctx: Signal<Option<DesktopContext>> = use_signal(|| None);

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

                let pill_w = (220.0 * scale) as u32;
                let pill_h = (52.0 * scale) as u32;
                let x = (monitor_size.width.saturating_sub(pill_w)) / 2;
                let y = monitor_size.height.saturating_sub(pill_h + (60.0 * scale) as u32);

                let builder = WindowBuilder::new()
                    .with_title("Beamer Recording")
                    .with_decorations(false)
                    .with_transparent(true)
                    .with_always_on_top(true)
                    .with_visible(false)
                    .with_focusable(false)
                    .with_inner_size(PhysicalSize::new(pill_w, pill_h))
                    .with_position(PhysicalPosition::new(x as i32, y as i32))
                    .with_skip_taskbar(true);

                let cfg = DesktopConfig::new()
                    .with_window(builder)
                    .with_background_color((0, 0, 0, 0))
                    .with_custom_head(format!("<style>{}</style>", PILL_CSS))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(RecordingPill);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;
                let _ = ctx.set_ignore_cursor_events(true);
                pill_ctx.set(Some(ctx));
            });
        }
    });

    // Show/hide the pill when recording state changes
    use_effect(move || {
        let recording = *is_recording.read();
        let pill_enabled = config.read().appearance.pill_enabled;
        if let Some(ctx) = pill_ctx.read().as_ref() {
            ctx.set_visible(recording && pill_enabled);
        }
    });

    let coroutine = use_coroutine(move |rx: UnboundedReceiver<HotkeyEvent>| {
        orchestrator::run(
            rx,
            config,
            is_recording,
            overlay_text,
            last_injection,
            history,
            status_log,
        )
    });

    // Re-registers the global hotkey whenever config changes (reactive via use_effect)
    let window_for_shortcut = window.clone();
    let mut shortcut_handle: Signal<Option<ShortcutHandle>> = use_signal(|| None);

    use_effect(move || {
        let cfg = config.read();
        let hotkey_str = cfg.recording.hotkey.clone();
        let is_toggle = cfg.recording.mode == "toggle";
        drop(cfg);

        if let Some(old) = shortcut_handle.write().take() {
            window_for_shortcut.remove_shortcut(old);
        }

        let hotkey = match hotkey_str.parse::<global_hotkey::hotkey::HotKey>() {
            Ok(hk) => hk,
            Err(e) => {
                tracing::error!("Failed to parse hotkey '{}': {:?}", hotkey_str, e);
                return;
            }
        };

        let mut toggled = false;
        match window_for_shortcut.create_shortcut(hotkey, move |state| {
            if is_toggle {
                if state == HotKeyState::Pressed {
                    toggled = !toggled;
                    if toggled {
                        coroutine.send(HotkeyEvent::RecordStart);
                    } else {
                        coroutine.send(HotkeyEvent::RecordStop);
                    }
                }
            } else {
                match state {
                    HotKeyState::Pressed => coroutine.send(HotkeyEvent::RecordStart),
                    HotKeyState::Released => coroutine.send(HotkeyEvent::RecordStop),
                }
            }
        }) {
            Ok(handle) => {
                shortcut_handle.set(Some(handle));
            }
            Err(e) => {
                tracing::error!("Failed to register hotkey '{}': {:?}", hotkey_str, e);
            }
        }
    });

    use_tray_menu_event_handler({
        let quit_id = items.quit.id().clone();
        let settings_id = items.settings.id().clone();
        let window = window.clone();
        move |event| {
            if event.id == quit_id {
                std::process::exit(0);
            } else if event.id == settings_id {
                current_page.set(Page::Settings);
                window.set_visible(true);
                window.set_focus();
            }
        }
    });

    use_tray_icon_event_handler({
        let window = window.clone();
        move |event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if window.is_visible() {
                    window.set_visible(false);
                } else {
                    window.set_visible(true);
                    window.set_focus();
                }
            }
        }
    });

    let page = *current_page.read();

    rsx! {
        head {
            link { rel: "stylesheet", href: asset!("assets/styles.css") }
        }
        div { class: "app-container",
            div { class: "titlebar",
                img {
                    class: "titlebar-icon",
                    src: asset!("assets/icon.png"),
                    alt: "Beamer",
                    width: "20",
                    height: "20",
                }
                div { class: "titlebar-controls",
                    button {
                        class: "titlebar-btn minimize",
                        onclick: {
                            let window = window.clone();
                            move |_| window.set_minimized(true)
                        },
                        IconMinus { size: 14 }
                    }
                    button {
                        class: "titlebar-btn close",
                        onclick: {
                            let window = window.clone();
                            move |_| window.set_visible(false)
                        },
                        IconX { size: 14 }
                    }
                }
            }
            div { class: "app-body",
                nav { class: "sidebar",
                    div { class: "sidebar-top",
                        button {
                            class: if page == Page::Home { "sidebar-icon active" } else { "sidebar-icon" },
                            onclick: move |_| current_page.set(Page::Home),
                            IconHouse {}
                        }
                        button {
                            class: if page == Page::History { "sidebar-icon active" } else { "sidebar-icon" },
                            onclick: move |_| current_page.set(Page::History),
                            IconClockCounterClockwise {}
                        }
                    }
                    div { class: "sidebar-bottom",
                        button {
                            class: if page == Page::Settings { "sidebar-icon active" } else { "sidebar-icon" },
                            onclick: move |_| current_page.set(Page::Settings),
                            IconGear {}
                        }
                    }
                }
                match page {
                    Page::Home => rsx! {
                        HomePage {
                            is_recording,
                            history,
                            config,
                        }
                    },
                    Page::History => rsx! {
                        HistoryPage { history }
                    },
                    Page::Settings => rsx! {
                        SettingsPage { config, last_injection, status_log }
                    },
                }
            }
        }
    }
}

const PILL_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; }
html, body, #main { background:transparent!important; overflow:hidden;
  font-family: "Segoe UI Variable","Segoe UI",system-ui,sans-serif; }

.pill { display:flex; align-items:center; gap:10px; padding:0 16px;
  height:44px; margin:4px auto; width:fit-content;
  background:rgba(15,15,20,0.88); border-radius:22px;
  border:1.5px solid rgba(255,255,255,0.08); }

.pill-dot { width:8px; height:8px; border-radius:50%; background:#DC2626;
  flex-shrink:0; animation:dot-pulse 1.5s ease-in-out infinite; }
@keyframes dot-pulse { 0%,100%{opacity:1} 50%{opacity:0.35} }

.pill-bars { display:flex; align-items:center; gap:3px; height:24px; }
.bar { width:3px; border-radius:1.5px; background:#9B6DFF;
  animation:wave 1.2s ease-in-out infinite; }
.bar-1{height:8px;  animation-delay:0s}
.bar-2{height:16px; animation-delay:.15s}
.bar-3{height:24px; animation-delay:.3s}
.bar-4{height:16px; animation-delay:.45s}
.bar-5{height:8px;  animation-delay:.6s}
@keyframes wave { 0%,100%{transform:scaleY(.4)} 50%{transform:scaleY(1)} }

.pill-label { font-size:13px; font-weight:500;
  color:rgba(255,255,255,0.75); letter-spacing:.01em; user-select:none; }
"#;
