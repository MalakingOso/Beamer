use dioxus::prelude::*;
use crate::ui::settings::layout::SubSection;

#[component]
pub fn InjectionCard() -> Element {
    // Probed off the render thread: on Windows this runs Win32 calls
    // (`GetForegroundWindow`, `OpenProcess`) plus `Clipboard::new()`, on
    // Linux a `wtype` probe and a D-Bus handshake — none of which may run
    // inline in a render. Same spawn_blocking pattern as the GNOME probe below.
    let mut availability: Signal<
        Option<Vec<(&'static str, &'static str, Result<(), String>)>>,
    > = use_signal(|| None);
    use_hook(move || {
        spawn(async move {
            match tokio::task::spawn_blocking(crate::injection::check_availability).await {
                Ok(rows) => availability.set(Some(rows)),
                Err(e) => tracing::warn!("injection availability probe panicked: {}", e),
            }
        });
    });
    let mut show_fallbacks = use_signal(|| false);

    rsx! {
        SubSection { label: "Text Injection".to_string(),
            // GNOME focus helper — the primary injection path; only on GNOME Wayland
            {
                let is_gnome_wayland = std::env::var("XDG_CURRENT_DESKTOP")
                    .map(|d| d.to_ascii_uppercase().contains("GNOME"))
                    .unwrap_or(false)
                    && std::env::var("XDG_SESSION_TYPE").ok().as_deref() == Some("wayland");

                #[cfg(not(target_os = "windows"))]
                {
                    use crate::install::gnome_extension::{self, Status as HelperStatus};

                    // Hooks must run unconditionally to satisfy Dioxus's hook
                    // ordering contract, so they sit outside the desktop check.
                    //
                    // `None` = the probe hasn't answered yet. Every call into
                    // `gnome_extension` shells out to `gnome-extensions` (the
                    // status probe alone runs ~110ms; install additionally
                    // copies files), so they all go through `spawn_blocking`
                    // rather than running inline in a render or an event
                    // handler, where they would freeze the window.
                    let mut status: Signal<Option<HelperStatus>> = use_signal(|| None);
                    let mut busy = use_signal(|| false);

                    use_hook(move || {
                        spawn(async move {
                            match tokio::task::spawn_blocking(gnome_extension::status).await {
                                Ok(s) => status.set(Some(s)),
                                Err(e) => tracing::warn!("GNOME extension status probe panicked: {}", e),
                            }
                        });
                    });

                    if is_gnome_wayland {
                        let current = *status.read();
                        let is_busy = *busy.read();
                        let (label, action): (String, Option<&str>) = match current {
                            None =>
                                ("GNOME helper: checking\u{2026}".into(), None),
                            Some(HelperStatus::Enabled) =>
                                ("GNOME helper: Active (direct typing)".into(), Some("Remove")),
                            Some(HelperStatus::Disabled) =>
                                ("GNOME helper: Installed, click to enable".into(), Some("Enable")),
                            Some(HelperStatus::PendingRestart) =>
                                ("GNOME helper: Installed — log out and back in to activate".into(), None),
                            Some(HelperStatus::NotInstalled) =>
                                ("Install GNOME helper for reliable typing + recording pill".into(), Some("Install")),
                            Some(HelperStatus::UpdateAvailable) =>
                                ("GNOME helper: update available (adds direct typing + pill)".into(), Some("Update")),
                            Some(HelperStatus::UpdatePendingRestart) =>
                                ("GNOME helper: updated — log out and back in to activate".into(), None),
                        };

                        rsx! {
                            div { class: "card-row",
                                span { class: "card-label", "{label}" }
                                if let (Some(btn), Some(current)) = (action, current) {
                                    button {
                                        class: "btn-small",
                                        disabled: is_busy,
                                        onclick: move |_| {
                                            if *busy.peek() {
                                                return;
                                            }
                                            busy.set(true);
                                            spawn(async move {
                                                let outcome = tokio::task::spawn_blocking(move || {
                                                    let result = match current {
                                                        HelperStatus::NotInstalled
                                                        | HelperStatus::UpdateAvailable =>
                                                            gnome_extension::install(),
                                                        HelperStatus::Disabled =>
                                                            gnome_extension::enable_installed(),
                                                        HelperStatus::Enabled =>
                                                            gnome_extension::uninstall(),
                                                        // These render no button; match is exhaustive
                                                        // for safety if the render and click race.
                                                        HelperStatus::PendingRestart
                                                        | HelperStatus::UpdatePendingRestart => Ok(()),
                                                    };
                                                    // Re-probe on the same blocking thread so the
                                                    // UI never sees a stale status.
                                                    (result, gnome_extension::status())
                                                })
                                                .await;

                                                match outcome {
                                                    Ok((result, fresh)) => {
                                                        if let Err(e) = result {
                                                            tracing::warn!("GNOME extension action failed: {}", e);
                                                        }
                                                        status.set(Some(fresh));
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!("GNOME extension action panicked: {}", e);
                                                    }
                                                }
                                                busy.set(false);
                                            });
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
                if let Some(rows) = availability.read().clone() {
                    div { class: "injection-fallbacks",
                        for (name, display, result) in rows.iter().filter(|(n, _, _)| *n != "gnome") {
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
                } else {
                    div { class: "injection-fallbacks", "Checking availability…" }
                }
            }
        }
    }
}
