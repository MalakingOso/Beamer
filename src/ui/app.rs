//! The root component of the main window. `App()` owns every long-lived piece
//! of app state as a `Signal` (config, recording state, history, notes, tasks,
//! status log, ...) and hands them down as props; there is no context provider.
//! It also starts the long-lived workers: the orchestrator coroutine fed by the
//! keyboard hook, the note-pipeline coroutine, the sticky-window reconciler, the
//! notes flush tick, live sync and the component reconcile. Then it renders the
//! sidebar + page shell.
//!
//! Hook order is fixed: each `setup_*`/`use_*` call runs once per render, in the
//! same order, never behind a runtime condition. Sticky windows are separate
//! `VirtualDom`s that receive these signals as props. See
//! `agent_docs/dioxus_architecture.md` and `agent_docs/sticky_notes.md`.

use std::rc::Rc;

use dioxus::desktop::use_window;
use dioxus::prelude::*;

use crate::config::Config;
use crate::hotkey::{start_ll_hook, CaptureMode, HotkeyConfig, HotkeyEvent};
use crate::notes::pipeline;
use crate::notes::sync_client;
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::orchestrator::{self, RecordingState};
use crate::update::UpdateStatus;
use crate::ui::{app_menu, app_setup, app_splash};
// Linux swaps the tray icon instead of opening a pill window, so nothing in
// `app_pill` compiles there (see the gated call site below).
#[cfg(not(target_os = "linux"))]
use crate::ui::app_pill;
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

/// Which page the main window shows (sidebar selection, also set by tray items).
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

    app_setup::setup_window_centering(window.clone());

    // Cold-start warmup splash. `app_ready` flips true when it closes; restored
    // stickies wait on it so they don't pop over the loading screen.
    let app_ready = use_signal(|| false);
    app_splash::setup_splash(window.clone(), app_ready);

    let mut current_page = use_signal(|| Page::Home);
    let rec_state = use_signal(RecordingState::default);
    let last_injection = use_signal(|| "No injection yet".to_string());
    let history = use_signal(TranscriptionHistory::load);
    let notes = use_signal(NoteStore::load);
    // After the notes, from the same automerge document (the note store owns the handle).
    let tasks = use_signal(|| TaskStore::load_beside(&notes.peek()));
    // Must precede `Config::load()`, which creates a missing file: the only point
    // that can still tell a fresh install from an upgrade. An `Err` counts as
    // "not fresh". The component reconcile below auto-accepts prompted downloads
    // (the model) on a fresh install; an upgrade asks in Settings.
    let fresh_install = matches!(Config::config_path().try_exists(), Ok(false));
    let config = use_signal(|| Config::load().unwrap_or_default());
    let status_log = use_signal(StatusLog::new);
    app_setup::report_load_errors(notes, tasks, status_log);
    let update_status = use_signal(UpdateStatus::default);
    // Which hotkey started the current recording, so the pill can say so.
    let active_mode = use_signal(CaptureMode::default);

    // Linux replaces the pill with an AppIndicator tray-icon swap (see
    // `linux_integration`): no in-tray GTK widgets or floating pill under Wayland.
    #[cfg(not(target_os = "linux"))]
    app_pill::setup_recording_pill(window.clone(), rec_state, active_mode, config);

    #[cfg(target_os = "linux")]
    linux_integration::setup_linux_integration(rec_state, active_mode, config);

    // The model pass lives here, not in the requesting window: `App()`'s scope
    // outlives every sticky, so closing a note mid-pass cannot cancel it.
    let (note_passes, note_passes_in_flight) = pipeline::use_pipeline(config, notes, tasks, status_log);

    let coroutine = use_coroutine(move |rx: UnboundedReceiver<HotkeyEvent>| {
        orchestrator::run(
            rx,
            config,
            rec_state,
            last_injection,
            history,
            status_log,
            notes,
            tasks,
            active_mode,
            note_passes,
        )
    });

    // Low-level keyboard hook (supports Win-key combos); its events are forwarded
    // into the orchestrator coroutine.
    let hotkey_handle = use_hook(move || {
        let (hook_tx, mut hook_rx) = tokio::sync::mpsc::unbounded_channel::<HotkeyEvent>();

        let cfg = config.peek();
        let initial = HotkeyConfig::parse(&cfg.recording.hotkey, cfg.recording.mode == "toggle")
            .unwrap_or_else(|| {
                // Unlike the note hotkey, the dictation hotkey falls back to
                // Ctrl+Space rather than "unbound", so recording stays reachable.
                tracing::warn!(
                    hotkey = %cfg.recording.hotkey,
                    "dictation hotkey failed to parse; falling back to the default Ctrl+Space"
                );
                HotkeyConfig::default()
            });
        let note_binding = cfg.recording.note_hotkey_config();
        drop(cfg);

        let handle = Rc::new(start_ll_hook(initial, note_binding, hook_tx));

        spawn(async move {
            while let Some(event) = hook_rx.recv().await {
                coroutine.send(event);
            }
        });

        handle
    });

    use_effect(move || {
        let cfg = config.read();
        let new_config = HotkeyConfig::parse(&cfg.recording.hotkey, cfg.recording.mode == "toggle")
            .unwrap_or_else(|| {
                // Same fallback as above: a bad dictation string must not stall
                // the note binding from reaching `update_configs`.
                tracing::warn!(
                    hotkey = %cfg.recording.hotkey,
                    "dictation hotkey failed to parse; falling back to the default Ctrl+Space"
                );
                HotkeyConfig::default()
            });
        hotkey_handle.update_configs(new_config, cfg.recording.note_hotkey_config());
    });

    // Keeps open windows matching notes that should be showing (fresh and restored).
    let sticky_registry = sticky_windows::setup_sticky_windows(
        window.clone(), notes, tasks, note_passes, note_passes_in_flight, config, app_ready,
    );

    // Coalesce per-keystroke edits into one write. Captured transcripts flush
    // immediately in `do_note_capture`; this tick only carries body/colour/geometry.
    app_setup::setup_notes_flush(notes, tasks);

    // Live sync, off unless `config.sync.url` names a server. The handle feeds
    // `SyncCard`'s live state without a second connection. `sync_doc` is its own
    // statement: `use_sync_client` writes `notes`, and an inline `peek()` would
    // hold its borrow past that write.
    let sync_doc = notes.peek().sync_doc();
    let sync_client_handle = sync_client::use_sync_client(config, sync_doc, notes, tasks);

    app_setup::setup_update_check(config, update_status);
    // Bring everything outside the exe (runtime, preset, task, model) in line
    // with this build's catalog. Silent unless a big download needs an OK.
    let components = crate::components::use_components(fresh_install);

    // Start Menu shortcut so toasts show under Beamer's own name (see `windows_shortcut`).
    #[cfg(target_os = "windows")]
    app_setup::setup_windows_aumid_shortcut();

    app_menu::setup_menu_handlers(&items, window.clone(), current_page, last_injection, config, update_status, notes, tasks);
    app_menu::setup_notes_tray_label(&items, notes);
    app_menu::setup_tray_click_handler(window.clone());

    let page = *current_page.read();

    rsx! {
        div { class: "app-container",
            div { class: "left-column",
                div { class: "corner-badge",
                    img {
                        class: "corner-badge-icon",
                        src: crate::assets::icon_png_data_url(),
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
            div { class: "right-column",
                div {
                    class: "titlebar",
                    // `-webkit-app-region:drag` fails on Linux (WebKitGTK): drag via onmousedown.
                    onmousedown: {
                        let window = window.clone();
                        move |_| { let _ = window.drag_window(); }
                    },
                    div { class: "titlebar-controls",
                        button {
                            class: "titlebar-btn minimize",
                            // Else the titlebar drag grabs the pointer and swallows the click.
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
                        NotesPage { notes, tasks, registry: sticky_registry }
                    },
                    Page::Tasks => rsx! {
                        TasksPage { notes, tasks, registry: sticky_registry }
                    },
                    Page::Vocab => rsx! {
                        VocabPage {}
                    },
                    Page::Settings => rsx! {
                        SettingsPage { config, last_injection, status_log, update_status, notes, tasks, sync_client: sync_client_handle, components }
                    },
                }
            }
        }
    }
}
