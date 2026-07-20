use dioxus::prelude::*;
use crate::ui::components::Card;

#[component]
pub fn InjectionCard() -> Element {
    let availability = use_hook(|| crate::injection::check_availability());
    let mut show_fallbacks = use_signal(|| false);

    rsx! {
        Card { title: "Text Injection".to_string(),
            // GNOME focus helper — the primary injection path; only on GNOME Wayland
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
                        use crate::install::gnome_extension::Status as HelperStatus;
                        let current = status();
                        let (label, action): (String, Option<&str>) = match current {
                            HelperStatus::Enabled =>
                                ("GNOME helper: Active (direct typing)".into(), Some("Remove")),
                            HelperStatus::Disabled =>
                                ("GNOME helper: Installed, click to enable".into(), Some("Enable")),
                            HelperStatus::PendingRestart =>
                                ("GNOME helper: Installed — log out and back in to activate".into(), None),
                            HelperStatus::NotInstalled =>
                                ("Install GNOME helper for reliable typing + recording pill".into(), Some("Install")),
                            HelperStatus::UpdateAvailable =>
                                ("GNOME helper: update available (adds direct typing + pill)".into(), Some("Update")),
                            HelperStatus::UpdatePendingRestart =>
                                ("GNOME helper: updated — log out and back in to activate".into(), None),
                        };

                        rsx! {
                            div { class: "card-row",
                                span { class: "card-label", "{label}" }
                                if let Some(btn) = action {
                                    button {
                                        class: "btn-small",
                                        onclick: move |_| {
                                            let result = match current {
                                                HelperStatus::NotInstalled
                                                | HelperStatus::UpdateAvailable =>
                                                    crate::install::gnome_extension::install(),
                                                HelperStatus::Disabled =>
                                                    crate::install::gnome_extension::enable_installed(),
                                                HelperStatus::Enabled =>
                                                    crate::install::gnome_extension::uninstall(),
                                                // These render no button; match is exhaustive
                                                // for safety if the render and click race.
                                                HelperStatus::PendingRestart
                                                | HelperStatus::UpdatePendingRestart => Ok(()),
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

            // Read-only availability of the fallback chain, collapsed by
            // default behind a disclosure row. The order is fixed (config
            // `injection.backends` is still honored if hand-edited in
            // config.toml); the helper row above covers the "gnome" backend.
            // The paste shortcut for the clipboard backend has no UI either —
            // `injection.paste_shortcut` stays on "auto" unless hand-edited.
            button {
                class: "injection-fallbacks-toggle",
                onclick: move |_| show_fallbacks.toggle(),
                span { class: "injection-fallbacks-chevron",
                    if show_fallbacks() { "\u{25BE}" } else { "\u{25B8}" }
                }
                "Fallbacks"
            }
            if show_fallbacks() {
                div { class: "injection-fallbacks",
                    for (name, display, result) in availability.iter().filter(|(n, _, _)| *n != "gnome") {
                        span {
                            key: "{name}",
                            class: "injection-fallback-item",
                            span {
                                class: if result.is_ok() { "status-dot ok" } else { "status-dot err" },
                                title: match result {
                                    Ok(()) => "Available".to_string(),
                                    Err(e) => e.clone(),
                                },
                            }
                            "{display}"
                        }
                    }
                }
            }
        }
    }
}
