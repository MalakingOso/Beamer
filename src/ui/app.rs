use std::rc::Rc;

use dioxus::desktop::use_window;
use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::{start_ll_hook, CaptureMode, HotkeyConfig, HotkeyEvent};
use crate::notes::pipeline;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::orchestrator::{self, RecordingState};
use crate::update::UpdateStatus;
use crate::ui::app_setup;
use crate::ui::sticky_windows;
#[cfg(target_os = "linux")]
use crate::ui::linux_integration;
use crate::ui::history::TranscriptionHistory;
use crate::ui::history_page::HistoryPage;
use crate::ui::home::HomePage;
use crate::ui::icons::{
    IconBook, IconClockCounterClockwise, IconGear, IconHouse, IconListChecks, IconMinus,
    IconNote, IconX,
};
use crate::ui::notes_page::NotesPage;
use crate::ui::tasks_page::TasksPage;
use crate::ui::vocab_page::VocabPage;
use crate::ui::settings::SettingsPage;
use crate::ui::status_log::StatusLog;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Page {
    Home,
    History,
    Notes,
    Tasks,
    Vocab,
    Settings,
}

#[component]
pub fn App() -> Element {
    let items = app_setup::setup_tray_menu();

    let window = use_window();

    // Center the window on the primary monitor (runs once on first render)
    app_setup::setup_window_centering(window.clone());

    // Cold-start warmup splash. Runs once on first render: opens a small
    // centered window, walks `warm_all` through keyring/audio/(mpris)/network,
    // then closes itself. Pays the one-time costs that would otherwise stall
    // the first recording.
    //
    // `app_ready` flips true when the splash closes; sticky notes restored
    // from disk wait on it so they don't pop up over the loading screen.
    let app_ready = use_signal(|| false);
    app_setup::setup_splash(window.clone(), app_ready);

    let mut current_page = use_signal(|| Page::Home);
    let rec_state = use_signal(RecordingState::default);
    let last_injection = use_signal(|| "No injection yet".to_string());
    let history = use_signal(TranscriptionHistory::load);
    let notes = use_signal(NoteStore::load);
    let tasks = use_signal(TaskStore::load);
    let config = use_signal(|| Config::load().unwrap_or_default());
    let status_log = use_signal(StatusLog::new);
    let update_status = use_signal(UpdateStatus::default);
    // Which hotkey started the current recording, so the pill can say so.
    let active_mode = use_signal(CaptureMode::default);

    // Recording pill window — small, transparent, click-through, always-on-top.
    // Linux: the pill is replaced by an AppIndicator tray-icon swap (see
    // `linux_integration`). GNOME Shell doesn't accept in-tray GTK widgets
    // from standalone apps, and the floating-pill approach has
    // compositor/transparency quirks under Wayland.
    #[cfg(not(target_os = "linux"))]
    app_setup::setup_recording_pill(window.clone(), rec_state, config);

    // Linux: swap the tray icon to reflect recording state and pump levels
    // into the shell pill (mirrors Handy's behavior).
    #[cfg(target_os = "linux")]
    linux_integration::setup_linux_integration(rec_state, active_mode, config);

    // The model passes live here rather than in the window that asked for
    // them: `App()`'s scope outlives every sticky, so closing a note mid-pass
    // cannot cancel it. Created before the orchestrator so its handle can be
    // threaded into the capture path.
    let note_passes = pipeline::use_pipeline(config, notes, tasks, status_log);

    let coroutine = use_coroutine(move |rx: UnboundedReceiver<HotkeyEvent>| {
        orchestrator::run(
            rx,
            config,
            rec_state,
            last_injection,
            history,
            status_log,
            notes,
            active_mode,
            note_passes,
        )
    });

    // Low-level keyboard hook for hotkey detection (supports Win key combos)
    let hotkey_handle = use_hook(move || {
        let (hook_tx, mut hook_rx) = tokio::sync::mpsc::unbounded_channel::<HotkeyEvent>();

        let cfg = config.peek();
        let initial = HotkeyConfig::parse(&cfg.recording.hotkey, cfg.recording.mode == "toggle")
            .unwrap_or_else(|| {
                // Unlike `note_hotkey_config()`, a `None` here does not mean
                // "unbound": the dictation hotkey always falls back to a
                // working default (Ctrl+Space) rather than leaving recording
                // unreachable. That fallback is silent unless logged: a
                // hand-edited config like "Ctrl+Super+Space" now fails to
                // parse (Super paired with another key is rejected) and
                // quietly rebinds to Ctrl+Space instead.
                tracing::warn!(
                    hotkey = %cfg.recording.hotkey,
                    "dictation hotkey failed to parse; falling back to the default Ctrl+Space"
                );
                HotkeyConfig::default()
            });
        let note_binding = cfg.recording.note_hotkey_config();
        drop(cfg);

        let handle = Rc::new(start_ll_hook(initial, note_binding, hook_tx));

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
            hotkey_handle.update_configs(new_config, cfg.recording.note_hotkey_config());
        }
    });

    // Sticky note windows: one effect keeps the set of open windows matching
    // the set of notes that should be showing. Covers both a note dictated just
    // now and notes restored from disk at startup.
    let sticky_registry = sticky_windows::setup_sticky_windows(window.clone(), notes, tasks, note_passes, config, app_ready);

    // Coalesce per-keystroke note edits into one write. `do_note_capture`
    // flushes a newly captured transcript immediately — that one must never be
    // lost — so this tick only ever carries body/colour/geometry edits.
    app_setup::setup_notes_flush(notes, tasks);

    // Background update check on startup (3s delay to keep launch snappy)
    app_setup::setup_update_check(config, update_status);

    // Tray menu clicks + tray icon left-click (toggle window visibility).
    app_setup::setup_menu_handlers(&items, window.clone(), current_page, last_injection, config, update_status, notes, tasks);
    app_setup::setup_tray_click_handler(window.clone());

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
                            class: if page == Page::Notes { "sidebar-icon active" } else { "sidebar-icon" },
                            onclick: move |_| current_page.set(Page::Notes),
                            IconNote {}
                        }
                        button {
                            // Directly after Notes: a task is only ever reached
                            // through the note that produced it, so the two
                            // read as one pair.
                            class: if page == Page::Tasks { "sidebar-icon active" } else { "sidebar-icon" },
                            onclick: move |_| current_page.set(Page::Tasks),
                            IconListChecks {}
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
                div {
                    class: "titlebar",
                    // -webkit-app-region:drag works on Windows (Chromium webview)
                    // but not on Linux (WebKitGTK). Use onmousedown to drag on all platforms.
                    onmousedown: {
                        let window = window.clone();
                        move |_| { let _ = window.drag_window(); }
                    },
                    div { class: "titlebar-controls",
                        button {
                            class: "titlebar-btn minimize",
                            // Prevent the titlebar's onmousedown from starting a window drag,
                            // which would grab the pointer and swallow the click (Linux/WebKitGTK).
                            onmousedown: move |e| e.stop_propagation(),
                            onclick: {
                                let window = window.clone();
                                move |_| window.set_minimized(true)
                            },
                            IconMinus { size: 14 }
                        }
                        button {
                            class: "titlebar-btn close",
                            onmousedown: move |e| e.stop_propagation(),
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
                    Page::Notes => rsx! {
                        NotesPage { notes, config, tasks, registry: sticky_registry }
                    },
                    Page::Tasks => rsx! {
                        TasksPage { notes, tasks, registry: sticky_registry }
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
