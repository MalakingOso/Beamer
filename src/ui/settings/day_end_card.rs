//! Settings → System → Day-end card: when the "here is what you got done" card
//! appears (see `ui::day_end`). The parent persists every change at once.

use dioxus::prelude::*;

use crate::ui::components::Toggle;
use crate::ui::settings::layout::SubSection;

/// Chip labels in `DayEndConfig::days` order: 1 = Monday .. 7 = Sunday.
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[derive(Props, Clone, PartialEq)]
pub struct DayEndSettingsProps {
    enabled: bool,
    time: String,
    days: Vec<u8>,
    fade_on: bool,
    /// Whether anything is logged today: with nothing there is no card to preview.
    can_preview: bool,
    on_enabled_change: EventHandler<bool>,
    on_time_change: EventHandler<String>,
    on_days_change: EventHandler<Vec<u8>>,
    on_fade_change: EventHandler<bool>,
    on_preview: EventHandler<()>,
}

#[component]
pub fn DayEndSettings(props: DayEndSettingsProps) -> Element {
    rsx! {
        SubSection { label: "Day-end card".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Show what I got done, at the end of the day" }
                Toggle {
                    value: props.enabled,
                    ontoggle: move |v: bool| props.on_enabled_change.call(v),
                }
            }
            if props.enabled {
                div { class: "card-row",
                    span { class: "card-label", "At" }
                    input {
                        class: "input input-time",
                        r#type: "time",
                        value: "{props.time}",
                        // `onchange`: fires once a full time is picked, not per keystroke.
                        onchange: move |e: Event<FormData>| props.on_time_change.call(e.value().to_string()),
                    }
                }
                div { class: "card-row",
                    span { class: "card-label", "On" }
                    div { class: "day-chips",
                        for (i, label) in DAYS.iter().enumerate() {
                            {
                                let n = (i + 1) as u8;
                                let on = props.days.contains(&n);
                                let days = props.days.clone();
                                rsx! {
                                    button {
                                        key: "{n}",
                                        class: if on { "day-chip on" } else { "day-chip" },
                                        aria_pressed: "{on}",
                                        onclick: move |_| {
                                            let mut next = days.clone();
                                            if on {
                                                next.retain(|d| *d != n);
                                            } else {
                                                next.push(n);
                                                next.sort_unstable();
                                            }
                                            props.on_days_change.call(next);
                                        },
                                        "{label}"
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "card-row",
                    div {
                        span { class: "card-label", "Fade away after 30 seconds" }
                        div { class: "card-label-hint", "Off: the card stays until you click it." }
                    }
                    Toggle {
                        value: props.fade_on,
                        ontoggle: move |v: bool| props.on_fade_change.call(v),
                    }
                }
                div { class: "card-row",
                    span { class: "card-label-hint",
                        if props.can_preview {
                            "See it with today's list."
                        } else {
                            "Nothing is logged yet today, so there is nothing to show."
                        }
                    }
                    button {
                        class: "btn btn-small",
                        disabled: !props.can_preview,
                        onclick: move |_| props.on_preview.call(()),
                        "Show it now"
                    }
                }
            }
        }
    }
}
