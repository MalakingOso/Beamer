use dioxus::prelude::*;
use crate::ui::components::Card;

#[derive(Props, Clone, PartialEq)]
pub struct InjectionCardProps {
    pub backends: Vec<String>,
    pub on_backends_change: EventHandler<Vec<String>>,
}

#[component]
pub fn InjectionCard(props: InjectionCardProps) -> Element {
    let availability = use_hook(|| crate::injection::check_availability());

    // Build list of backends NOT currently in the config (for "Add" dropdown)
    let all_names: Vec<(&str, &str)> = availability
        .iter()
        .map(|(name, display, _)| (*name, *display))
        .collect();
    let unused: Vec<(&str, &str)> = all_names
        .iter()
        .filter(|(name, _)| !props.backends.contains(&name.to_string()))
        .copied()
        .collect();

    rsx! {
        Card { title: "Text Injection".to_string(),
            // Ordered backend list
            for (i, name) in props.backends.iter().enumerate() {
                {
                    let avail = availability.iter().find(|(n, _, _)| *n == name.as_str());
                    let display = avail.map(|(_, d, _)| *d).unwrap_or(name.as_str());
                    let is_ok = avail.map(|(_, _, r)| r.is_ok()).unwrap_or(false);
                    let err_msg = avail
                        .and_then(|(_, _, r)| r.as_ref().err().cloned())
                        .unwrap_or_default();

                    rsx! {
                        div { class: "card-row",
                            div { class: "injection-backend-row",
                                span {
                                    class: if is_ok { "status-dot ok" } else { "status-dot err" },
                                    title: if is_ok { "Available" } else { "{err_msg}" },
                                }
                                span { class: "card-label", "{display}" }
                            }
                            div { class: "injection-backend-controls",
                                if i > 0 {
                                    button {
                                        class: "btn-icon",
                                        title: "Move up",
                                        onclick: {
                                            let mut list = props.backends.clone();
                                            move |_| {
                                                list.swap(i, i - 1);
                                                props.on_backends_change.call(list.clone());
                                            }
                                        },
                                        "\u{25B2}"
                                    }
                                }
                                if i < props.backends.len() - 1 {
                                    button {
                                        class: "btn-icon",
                                        title: "Move down",
                                        onclick: {
                                            let mut list = props.backends.clone();
                                            move |_| {
                                                list.swap(i, i + 1);
                                                props.on_backends_change.call(list.clone());
                                            }
                                        },
                                        "\u{25BC}"
                                    }
                                }
                                button {
                                    class: "btn-icon remove",
                                    title: "Remove",
                                    onclick: {
                                        let mut list = props.backends.clone();
                                        move |_| {
                                            list.remove(i);
                                            props.on_backends_change.call(list.clone());
                                        }
                                    },
                                    "\u{00D7}"
                                }
                            }
                        }
                    }
                }
            }

            // Add backend dropdown
            if !unused.is_empty() {
                div { class: "card-row",
                    select {
                        class: "select",
                        onchange: {
                            let backends = props.backends.clone();
                            move |e: Event<FormData>| {
                                let val = e.value().to_string();
                                if !val.is_empty() {
                                    let mut list = backends.clone();
                                    list.push(val);
                                    props.on_backends_change.call(list);
                                }
                            }
                        },
                        option { value: "", selected: true, "Add backend\u{2026}" }
                        for (val, label) in &unused {
                            option { value: *val, "{label}" }
                        }
                    }
                    button {
                        class: "btn-small",
                        onclick: {
                            move |_| {
                                props.on_backends_change.call(
                                    crate::injection::default_backend_names()
                                );
                            }
                        },
                        "Reset"
                    }
                }
            }
        }
    }
}
