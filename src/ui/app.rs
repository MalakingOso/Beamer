use std::rc::Rc;

use dioxus::desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
use dioxus::desktop::tao::platform::windows::WindowBuilderExtWindows;
use dioxus::desktop::trayicon::{init_tray_icon, MouseButton, MouseButtonState, TrayIconEvent};
use dioxus::desktop::{
    use_tray_icon_event_handler, use_tray_menu_event_handler, use_window,
    Config as DesktopConfig, DesktopContext, WindowBuilder,
};
use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::{start_ll_hook, HotkeyConfig, HotkeyEvent};
use crate::orchestrator::{self, RecordingState};
use crate::tray;
use crate::update::{self, UpdateStatus};
use crate::ui::pill::RecordingPill;
use crate::ui::history::TranscriptionHistory;
use crate::ui::history_page::HistoryPage;
use crate::ui::home::HomePage;
use crate::ui::icons::{IconBook, IconClockCounterClockwise, IconGear, IconHouse, IconMinus, IconX};
use crate::ui::vocab_page::VocabPage;
use crate::ui::settings::SettingsPage;
use crate::ui::status_log::StatusLog;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    History,
    Vocab,
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

    // Center the window on the primary monitor (runs once on first render)
    use_hook({
        let window = window.clone();
        move || {
            if let Some(monitor) = window.primary_monitor() {
                let monitor_size = monitor.size();
                let scale = monitor.scale_factor();
                let win_w = (500.0 * scale) as i32;
                let win_h = (600.0 * scale) as i32;
                let x = (monitor_size.width as i32 - win_w) / 2;
                let y = (monitor_size.height as i32 - win_h) / 2;
                window.set_outer_position(PhysicalPosition::new(x, y));
            }
        }
    });

    let mut current_page = use_signal(|| Page::Home);
    let rec_state = use_signal(RecordingState::default);
    let overlay_text = use_signal(String::new);
    let last_injection = use_signal(|| "No injection yet".to_string());
    let history = use_signal(TranscriptionHistory::load);
    let config = use_signal(|| Config::load().unwrap_or_default());
    let status_log = use_signal(StatusLog::new);
    let mut update_status = use_signal(UpdateStatus::default);

    // Shared signals — consumed by child components
    use_context_provider(|| rec_state);
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
                    .with_data_directory(super::webview_data_dir())
                    .with_window(builder)
                    .with_background_color((0, 0, 0, 0))
                    .with_custom_head(format!(r#"<link href="https://fonts.googleapis.com/css2?family=DM+Mono:wght@400;500&display=swap" rel="stylesheet"><style>body{{opacity:0;transition:opacity 0.15s ease;}}{}</style>"#, PILL_CSS))
                    .with_exits_when_last_window_closes(false);

                let dom = VirtualDom::new(RecordingPill);
                let ctx: DesktopContext = window.new_window(dom, cfg).await;
                let _ = ctx.set_ignore_cursor_events(true);
                // Make the pill Win32-visible immediately (with CSS opacity:0 already
                // applied via custom head). This must happen after webview init so
                // transparency works. Once visible, parent show/hide won't trigger
                // WM_SHOWWINDOW flashes since the window is already shown.
                ctx.set_visible(true);
                pill_ctx.set(Some(ctx));
            });
        }
    });

    // Update pill appearance when recording state changes (CSS opacity + content swap).
    // Uses CSS opacity instead of Win32 visibility to avoid flash on parent show/hide.
    use_effect(move || {
        let state = *rec_state.read();
        let pill_enabled = config.read().appearance.pill_enabled;
        if let Some(ctx) = pill_ctx.read().as_ref() {
            let should_show = state != RecordingState::Idle && pill_enabled;
            if should_show {
                let (label, dot_class, bars_class) = match state {
                    RecordingState::Recording => {
                        ("Recording", "pill-dot", "pill-bars")
                    }
                    RecordingState::Processing => {
                        ("Processing", "pill-dot processing", "pill-bars processing")
                    }
                    _ => unreachable!(),
                };
                // Set content first, then fade in
                let _ = ctx.webview.evaluate_script(&format!(
                    r#"var d=document.querySelector('[class^="pill-dot"]');if(d)d.className='{dot_class}';
                       var b=document.querySelector('[class^="pill-bars"]');if(b)b.className='{bars_class}';
                       var l=document.querySelector('.pill-label');if(l)l.textContent='{label}';
                       document.body.style.opacity='1';"#,
                ));
            } else {
                // Just fade out — don't touch classes to avoid flash
                let _ = ctx.webview.evaluate_script(
                    "document.body.style.opacity='0';",
                );
            }
        }
    });

    let coroutine = use_coroutine(move |rx: UnboundedReceiver<HotkeyEvent>| {
        orchestrator::run(
            rx,
            config,
            rec_state,
            overlay_text,
            last_injection,
            history,
            status_log,
        )
    });

    // Low-level keyboard hook for hotkey detection (supports Win key combos)
    let hotkey_handle = use_hook(move || {
        let (hook_tx, mut hook_rx) = tokio::sync::mpsc::unbounded_channel::<HotkeyEvent>();

        let cfg = config.peek();
        let initial = HotkeyConfig::parse(&cfg.recording.hotkey, cfg.recording.mode == "toggle")
            .unwrap_or_default();
        drop(cfg);

        let handle = Rc::new(start_ll_hook(initial, hook_tx));

        // Bridge hook events to the orchestrator coroutine
        spawn(async move {
            while let Some(event) = hook_rx.recv().await {
                coroutine.send(event);
            }
        });

        handle
    });

    // Re-configure hotkey when config changes
    use_effect(move || {
        let cfg = config.read();
        if let Some(new_config) =
            HotkeyConfig::parse(&cfg.recording.hotkey, cfg.recording.mode == "toggle")
        {
            hotkey_handle.update_config(new_config);
        }
    });

    // Background update check on startup (3s delay to keep launch snappy)
    use_hook({
        let auto_check = config.peek().appearance.auto_check_updates;
        move || {
            if auto_check {
                spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    update_status.set(UpdateStatus::Checking);
                    let result = tokio::task::spawn_blocking(update::check_for_update_blocking).await;
                    match result {
                        Ok(Ok(Some(info))) => {
                            tracing::info!("Update available: v{}", info.version);
                            update_status.set(UpdateStatus::Available { version: info.version });
                        }
                        Ok(Ok(None)) => {
                            tracing::debug!("No update available");
                            update_status.set(UpdateStatus::Idle);
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("Update check failed: {}", e);
                            update_status.set(UpdateStatus::Idle);
                        }
                        Err(e) => {
                            tracing::warn!("Update check task panicked: {}", e);
                            update_status.set(UpdateStatus::Idle);
                        }
                    }
                });
            }
        }
    });

    use_tray_menu_event_handler({
        let quit_id = items.quit.id().clone();
        let settings_id = items.settings.id().clone();
        let check_updates_id = items.check_updates.id().clone();
        let window = window.clone();
        move |event| {
            if event.id == quit_id {
                std::process::exit(0);
            } else if event.id == settings_id {
                current_page.set(Page::Settings);
                window.set_visible(true);
                window.set_focus();
            } else if event.id == check_updates_id {
                spawn(async move {
                    update_status.set(UpdateStatus::Checking);
                    let result = tokio::task::spawn_blocking(update::check_for_update_blocking).await;
                    match result {
                        Ok(Ok(Some(info))) => {
                            update_status.set(UpdateStatus::Available { version: info.version });
                        }
                        Ok(Ok(None)) => {
                            update_status.set(UpdateStatus::Idle);
                        }
                        Ok(Err(e)) => {
                            update_status.set(UpdateStatus::Error(e.to_string()));
                        }
                        Err(e) => {
                            update_status.set(UpdateStatus::Error(format!("Task panicked: {e}")));
                        }
                    }
                });
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
            // Left column: badge + sidebar stacked vertically
            div { class: "left-column",
                div { class: "corner-badge",
                    img {
                        class: "corner-badge-icon",
                        src: asset!("assets/icon.png"),
                        alt: "Beamer",
                    }
                }
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
                        button {
                            class: if page == Page::Vocab { "sidebar-icon active" } else { "sidebar-icon" },
                            onclick: move |_| current_page.set(Page::Vocab),
                            IconBook {}
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
            }
            // Right column: titlebar + content stacked vertically
            div { class: "right-column",
                div { class: "titlebar",
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
                match page {
                    Page::Home => rsx! {
                        HomePage {
                            rec_state,
                            history,
                            config,
                        }
                    },
                    Page::History => rsx! {
                        HistoryPage { history }
                    },
                    Page::Vocab => rsx! {
                        VocabPage {}
                    },
                    Page::Settings => rsx! {
                        SettingsPage { config, last_injection, status_log, update_status }
                    },
                }
            }
        }
    }
}

const PILL_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; }
html, body, #main { background:transparent!important; overflow:hidden;
  font-family: "DM Mono","Segoe UI Variable","Segoe UI",monospace,system-ui,sans-serif; }

.pill { display:flex; align-items:center; gap:10px; padding:0 16px;
  height:44px; margin:4px auto; width:fit-content;
  background:rgba(255,255,255,0.92); border-radius:6px;
  border:2px solid rgba(75,0,130,0.12);
  box-shadow:2px 4px 0 0 rgba(75,0,130,0.12); }

.pill-dot { width:8px; height:8px; border-radius:50%; background:#DC2626;
  flex-shrink:0; animation:dot-pulse 1.5s ease-in-out infinite; }
.pill-dot.processing { display:none; }
@keyframes dot-pulse { 0%,100%{opacity:1} 50%{opacity:0.35} }

.pill-bars { display:flex; align-items:center; gap:3px; height:24px; }
.bar { width:3px; border-radius:1.5px; background:#4B0082;
  animation:wave 1.2s ease-in-out infinite; }
.bar-1{height:8px;  animation-delay:0s}
.bar-2{height:16px; animation-delay:.15s}
.bar-3{height:24px; animation-delay:.3s}
.bar-4{height:16px; animation-delay:.45s}
.bar-5{height:8px;  animation-delay:.6s}
@keyframes wave { 0%,100%{transform:scaleY(.4)} 50%{transform:scaleY(1)} }

.pill-bars.processing { gap:5px; align-items:center; }
.pill-bars.processing .bar {
  width:6px; height:6px; border-radius:50%;
  animation:bounce-dot 1.2s ease-in-out infinite; }
.pill-bars.processing .bar-1{animation-delay:0s}
.pill-bars.processing .bar-2{animation-delay:.15s}
.pill-bars.processing .bar-3{animation-delay:.3s}
.pill-bars.processing .bar-4,
.pill-bars.processing .bar-5{display:none}
@keyframes bounce-dot {
  0%,80%,100%{transform:translateY(0)}
  40%{transform:translateY(-8px)} }

.pill-label { font-size:13px; font-weight:500;
  color:#64708b; letter-spacing:.01em; user-select:none;
  font-family:"DM Mono",monospace; }
"#;
