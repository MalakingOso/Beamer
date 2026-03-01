use dioxus::desktop::trayicon::{init_tray_icon, MouseButton, MouseButtonState, TrayIconEvent};
use dioxus::desktop::{
    use_tray_icon_event_handler, use_tray_menu_event_handler, use_window, HotKeyState,
    ShortcutHandle,
};
use dioxus::prelude::*;
use std::ffi::c_void;

use crate::config::Config;
use crate::hotkey::HotkeyEvent;
use crate::orchestrator;
use crate::tray;
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

    // Enable acrylic system backdrop via DWM
    use_hook({
        let window = window.clone();
        move || {
            use dioxus::desktop::tao::platform::windows::WindowExtWindows;
            use windows::Win32::Foundation::HWND;
            use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};
            let hwnd_raw = window.hwnd() as isize;
            unsafe {
                let backdrop_type: i32 = 3; // DWMSBT_TRANSIENTWINDOW = Acrylic
                let _ = DwmSetWindowAttribute(
                    HWND(hwnd_raw as *mut c_void),
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &backdrop_type as *const i32 as *const c_void,
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    });

    let mut current_page = use_signal(|| Page::Home);
    let is_recording = use_signal(|| false);
    let overlay_text = use_signal(String::new);
    let last_injection = use_signal(|| "No injection yet".to_string());
    let history = use_signal(TranscriptionHistory::load);
    let config = use_signal(|| Config::load().unwrap_or_default());
    let status_log = use_signal(StatusLog::new);

    // Provide shared signals for child components / future multi-window use
    use_context_provider(|| is_recording);
    use_context_provider(|| overlay_text);
    use_context_provider(|| last_injection);
    use_context_provider(|| config);

    // Spawn the orchestration coroutine (hotkey → audio → transcribe → inject)
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

    // Register global hotkey — store handle so we can swap it on save
    let window_for_shortcut = window.clone();
    let mut shortcut_handle: Signal<Option<ShortcutHandle>> = use_signal(|| None);

    use_effect(move || {
        let cfg = config.read();
        let hotkey_str = cfg.recording.hotkey.clone();
        let is_toggle = cfg.recording.mode == "toggle";
        drop(cfg);

        // Remove old shortcut if any
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

    // Tray menu events (Settings / Quit)
    use_tray_menu_event_handler({
        let quit_id = items.quit.id().clone();
        let settings_id = items.settings.id().clone();
        let window = window.clone();
        move |event| {
            if event.id == quit_id {
                std::process::exit(0);
            } else if event.id == settings_id {
                if window.is_visible() {
                    window.set_visible(false);
                } else {
                    current_page.set(Page::Settings);
                    window.set_visible(true);
                    window.set_focus();
                }
            }
        }
    });

    // Tray icon left-click toggles window
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
                // Sidebar
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
                // Content
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
