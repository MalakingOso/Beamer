use dioxus::prelude::*;

use crate::ui::components::{Card, Toggle};
use crate::update::{self, UpdateStatus};

#[derive(Props, Clone, PartialEq)]
pub struct UpdateCardProps {
    pub update_status: Signal<UpdateStatus>,
    pub auto_check_updates: bool,
    pub on_auto_check_toggle: EventHandler<bool>,
}

#[component]
pub fn UpdateCard(props: UpdateCardProps) -> Element {
    let update_status = props.update_status;

    rsx! {
        Card { title: "Updates".to_string(),
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
                                // spawn_blocking so the closure itself returns ()
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
        }
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
