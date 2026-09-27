//! Settings → Sync & Updates → Updates: current version, auto-check toggle, and
//! the check → download → restart flow for self-update (`src/update/`). Also
//! lists install components (`crate::components`) that need a download
//! decision or failed.

use dioxus::prelude::*;

use crate::components::{Components, Status};
use crate::notes::task_store::TaskStore;
use crate::notes::NoteStore;
use crate::ui::components::Toggle;
use crate::ui::settings::layout::SubSection;
use crate::update::{self, UpdateStatus};

#[derive(Props, Clone, PartialEq)]
pub struct UpdateCardProps {
    pub update_status: Signal<UpdateStatus>,
    pub auto_check_updates: bool,
    pub on_auto_check_toggle: EventHandler<bool>,
    pub notes: Signal<NoteStore>,
    pub tasks: Signal<TaskStore>,
    pub components: Components,
}

#[component]
pub fn UpdateCard(props: UpdateCardProps) -> Element {
    let update_status = props.update_status;
    let mut notes = props.notes;
    let mut tasks = props.tasks;
    let components = props.components;
    let component_rows = components.attention();

    rsx! {
        SubSection { label: "Updates".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Current version" }
                span { class: "card-value", "v{update::current_version()}" }
            }
            div { class: "card-row",
                span { class: "card-label", "Check automatically" }
                Toggle {
                    value: props.auto_check_updates,
                    ontoggle: move |v: bool| props.on_auto_check_toggle.call(v),
                }
            }
            match &*update_status.read() {
                UpdateStatus::Idle => rsx! {
                    div { class: "card-row",
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| trigger_check(update_status),
                            "Check for Updates"
                        }
                    }
                },
                UpdateStatus::Checking => rsx! {
                    div { class: "card-row",
                        button {
                            class: "btn btn-secondary",
                            disabled: true,
                            "Checking..."
                        }
                    }
                },
                UpdateStatus::Available { version } => rsx! {
                    div { class: "card-row",
                        span { class: "card-label", "New version available: v{version}" }
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| trigger_apply(update_status),
                            "Download Update"
                        }
                    }
                },
                UpdateStatus::Downloading => rsx! {
                    div { class: "card-row",
                        button {
                            class: "btn btn-secondary",
                            disabled: true,
                            "Downloading..."
                        }
                    }
                },
                UpdateStatus::ReadyToRestart => rsx! {
                    div { class: "card-row",
                        span { class: "card-label", "Update downloaded" }
                        button {
                            class: "btn btn-primary",
                            onclick: move |_| {
                                // Flush here, on the render thread where `write()`
                                // is valid: `restart_app` never returns and its
                                // `process::exit` skips the flush tick, so unsaved
                                // note edits would be lost.
                                crate::notes::flush_stores(&mut notes.write(), &mut tasks.write());
                                std::thread::spawn(|| update::restart_app());
                            },
                            "Restart Now"
                        }
                    }
                },
                UpdateStatus::Error(msg) => rsx! {
                    div { class: "card-row",
                        span { class: "card-label", style: "color: var(--fg-secondary);", "{msg}" }
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| trigger_check(update_status),
                            "Retry"
                        }
                    }
                },
            }
            // Parts of the install outside the exe (`crate::components`).
            // Quiet unless one needs a decision or went wrong.
            if !component_rows.is_empty() {
                div { class: "card-row",
                    span { class: "card-label", "Components" }
                }
            }
            for (key, label, size, status) in component_rows {
                div { class: "card-row", key: "{key}",
                    match status {
                        Status::Pending => rsx! {
                            span { class: "card-label",
                                "{label}"
                                if let Some(size) = size { " \u{00b7} {human_size(size)}" }
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: move |_| components.accept(key),
                                "Download"
                            }
                        },
                        Status::Downloading { bytes, total } => {
                            let pct = bytes.saturating_mul(100).checked_div(total).unwrap_or(0);
                            rsx! {
                                span { class: "card-label", "{label} \u{00b7} {pct}%" }
                                button {
                                    class: "btn btn-secondary",
                                    onclick: move |_| components.cancel(),
                                    "Cancel"
                                }
                            }
                        },
                        Status::Failed(msg) => rsx! {
                            span { class: "card-label", style: "color: var(--fg-secondary);", "{label}: {msg}" }
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| components.run(),
                                "Retry"
                            }
                        },
                        _ => rsx! {},
                    }
                }
            }
        }
    }
}

/// `1_148_614_016` → "1.1 GB". Decimal units, as download sizes usually are.
fn human_size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    let mb = bytes as f64 / MB;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

fn trigger_check(mut update_status: Signal<UpdateStatus>) {
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
}

fn trigger_apply(mut update_status: Signal<UpdateStatus>) {
    spawn(async move {
        update_status.set(UpdateStatus::Downloading);
        let result = tokio::task::spawn_blocking(update::apply_update_blocking).await;
        match result {
            Ok(Ok(())) => {
                update_status.set(UpdateStatus::ReadyToRestart);
            }
            Ok(Err(e)) => {
                update_status.set(UpdateStatus::Error(format!("Download failed: {e}")));
            }
            Err(e) => {
                update_status.set(UpdateStatus::Error(format!("Task panicked: {e}")));
            }
        }
    });
}
