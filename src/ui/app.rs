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
use crate::ui::{app_menu, app_pill, app_setup, app_splash};
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

    app_setup::setup_window_centering(window.clone());

    // Cold-start warmup splash. `app_ready` flips true when it closes; restored
    // stickies wait on it so they don't pop over the loading screen.
    let app_ready = use_signal(|| false);
    app_splash::setup_splash(window.clone(), app_ready);

    let mut current_page = use_signal(|| Page::Home);
    let rec_state = use_signal(RecordingState::default);
    let last_injection = use_signal(|| "No injection yet".to_string());
    let history = use_signal(TranscriptionHistory::load);
    let mut notes = use_signal(NoteStore::load);
    // After the notes, from the same automerge document (the note store owns the handle).
    let tasks = use_signal(|| TaskStore::load_beside(&notes.peek()));
    // Taken *before* `Config::load()`, which creates the file as a side
    // effect when it's missing — this is the only point that can still tell
    // "fresh install" from "upgrade of an existing install". `Ok(false)`
    // only: an `Err` (can't tell) is treated as "not fresh", the same
    // conservative call `Config::load()` itself documents for the same stat.
    let fresh_install = matches!(Config::config_path().try_exists(), Ok(false));
    let config = use_signal(|| Config::load().unwrap_or_default());
    let download_status = use_signal(crate::model_setup::DownloadStatus::default);
    // K2-Horizon local extraction only exists for a bundled aarch64 build
    // (the only arch the fork's llama-server.exe is built for). A fresh
    // install downloads and starts it itself; an upgrade over an existing
    // install must not retroactively engage this — that case's file
    // placement/task update is instead handled entirely by the installer's
    // hooks.nsh. See agent_docs/local_inference.md.
    #[cfg(target_arch = "aarch64")]
    use_hook(move || {
        if fresh_install {
            crate::model_setup::spawn_ensure_model_present(download_status);
        }
    });
    // `use_hook`, not a plain call: `Signal::write` notifies every subscriber
    // even when the value is unchanged, and this flag is fixed for the process.
    // Must run before anything can delete an attachment (`release_attachment_bytes`
    // reads it). Same `should_start` check `sync_client` uses.
    use_hook(move || {
        notes.write().set_sync_enabled(sync_client::should_start(&config.peek().sync.url));
    });
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

    // Model passes live here, not in the requesting window: `App()`'s scope
    // outlives every sticky, so closing a note mid-pass cannot cancel it.
    let (note_passes, note_passes_in_flight) = pipeline::use_pipeline(config, notes, tasks, status_log);

    // Cleanup is opt-in and toggled independently of extraction (see
    // `llm::CleanupConfig::enabled`). With it off, notes carrying a `Failed`
    // from when it was on would keep reporting it forever: the pipeline only
    // ever visits a note it is asked about, and nothing asks about a pass that
    // no longer runs.
    //
    // Subscribes to `notes` as well as `config`, deliberately. Two writers add
    // notes this effect would otherwise never see: the `+` buttons, which
    // create a `Typed` note and send no pipeline request at all, and the sync
    // tick, which replaces `notes.notes` wholesale with whatever the document
    // hydrates to — including a note another machine failed to clean. Reading
    // rather than peeking costs one extra pass and cannot loop: the effect's
    // own write drives `has_cleanup_left_to_skip` to false, which is the fixed
    // point. The guard is what makes that true, and it also keeps an unrelated
    // config save from notifying every sticky window for nothing.
    use_effect(move || {
        let wanted = config.read().llm.cleanup_wanted();
        if !wanted && notes.read().has_cleanup_left_to_skip() {
            notes.write().skip_cleanup_on_every_note();
        }
    });

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

    // Low-level keyboard hook for hotkey detection (supports Win key combos)
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
    // `SyncCard`'s live state without a second connection.
    //
    // `sync_doc` is its own statement, not an inline argument: `use_sync_client`
    // writes `notes`, and an inline `peek()` would hold its borrow past that write.
    let sync_doc = notes.peek().sync_doc();
    let sync_client_handle = sync_client::use_sync_client(config, sync_doc, notes, tasks);

    app_setup::setup_update_check(config, update_status);

    // Start Menu shortcut so toasts show under Beamer's own name (see `windows_shortcut`).
    #[cfg(target_os = "windows")]
    app_setup::setup_windows_aumid_shortcut();

    app_menu::setup_menu_handlers(&items, window.clone(), current_page, last_injection, config, update_status, notes, tasks);
    app_menu::setup_notes_tray_label(&items, notes);
    app_menu::setup_tray_click_handler(window.clone());

    let page = *current_page.read();

    rsx! {
        head {
            link { rel: "stylesheet", href: asset!("/assets/styles.css") }
        }
        div { class: "app-container",
            div { class: "left-column",
                div { class: "corner-badge",
                    img {
                        class: "corner-badge-icon",
                        src: asset!("/assets/icon.png"),
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
                        SettingsPage { config, last_injection, status_log, update_status, notes, tasks, sync_client: sync_client_handle, download_status }
                    },
                }
            }
        }
    }
}
