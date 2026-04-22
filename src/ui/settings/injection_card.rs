use dioxus::prelude::*;
use crate::ui::components::{Card, Select};

#[derive(Props, Clone, PartialEq)]
pub struct InjectionCardProps {
    pub backends: Vec<String>,
    pub on_backends_change: EventHandler<Vec<String>>,
    #[props(default = String::from("auto"))]
    pub paste_shortcut: String,
    #[props(default)]
    pub on_paste_shortcut_change: EventHandler<String>,
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

            // Paste-shortcut override for the clipboard backend (Linux only).
            // "Auto" queries the GNOME focus helper extension (see section below)
            // to pick Ctrl+V or Ctrl+Shift+V per-app. If the helper isn't installed
            // we fall back to Ctrl+Shift+V — the universal terminal paste that also
            // degrades to "paste plain text" in most other apps.
            div { class: "card-row",
                span { class: "card-label", "Paste shortcut (Linux)" }
                Select {
                    value: props.paste_shortcut.clone(),
                    options: vec![
                        ("auto".into(), "Auto (recommended)".into()),
                        ("ctrl_shift_v".into(), "Ctrl+Shift+V".into()),
                        ("ctrl_v".into(), "Ctrl+V".into()),
                    ],
                    onchange: move |v: String| props.on_paste_shortcut_change.call(v),
                }
            }

            // GNOME focus helper — only show on GNOME Wayland
            {
                let is_gnome_wayland = std::env::var("XDG_CURRENT_DESKTOP")
                    .map(|d| d.to_ascii_uppercase().contains("GNOME"))
                    .unwrap_or(false)
                    && std::env::var("XDG_SESSION_TYPE").ok().as_deref() == Some("wayland");

                #[cfg(not(target_os = "windows"))]
                {
                    // Hook must be called unconditionally to satisfy Dioxus's hook ordering
                    // contract. The subprocess runs ~110ms on first render regardless of
                    // desktop — acceptable since it only happens once per Settings open on
                    // non-Windows platforms.
                    let mut status = use_signal(|| crate::install::gnome_extension::status());

                    if is_gnome_wayland {
                        let current = status();
                        let (label, action): (String, Option<&str>) = match current {
                            crate::install::gnome_extension::Status::Enabled =>
                                ("GNOME focus helper: Active".into(), Some("Remove")),
                            crate::install::gnome_extension::Status::Disabled =>
                                ("GNOME focus helper: Installed but not yet loaded — log out and back in".into(), None),
                            crate::install::gnome_extension::Status::NotInstalled =>
                                ("Install GNOME focus helper for app-aware pasting".into(), Some("Install")),
                        };

                        rsx! {
                            div { class: "card-row",
                                span { class: "card-label", "{label}" }
                                if let Some(btn) = action {
                                    button {
                                        class: "btn-small",
                                        onclick: move |_| {
                                            let result = match current {
                                                crate::install::gnome_extension::Status::NotInstalled =>
                                                    crate::install::gnome_extension::install(),
                                                _ => crate::install::gnome_extension::uninstall(),
                                            };
                                            if let Err(e) = result {
                                                tracing::warn!("GNOME extension action failed: {}", e);
                                            }
                                            status.set(crate::install::gnome_extension::status());
                                        },
                                        "{btn}"
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! { }
                    }
                }

                #[cfg(target_os = "windows")]
                {
                    let _ = is_gnome_wayland;
                    rsx! { }
                }
            }
        }
    }
}
