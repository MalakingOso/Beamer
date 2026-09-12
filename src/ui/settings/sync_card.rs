//! Settings card for the sync client: a toggle, one address field, and the
//! live connection state. Reads `status` (owned by `use_sync_client`); never
//! opens its own socket, which would register a second peer server-side.

use dioxus::prelude::*;

use crate::notes::sync_client::SyncStatus;
use crate::ui::components::Toggle;
use crate::ui::settings::layout::SubSection;

#[derive(Props, Clone, PartialEq)]
pub struct SyncCardProps {
    /// Empty means off (same convention as `note_hotkey`).
    pub url: String,
    /// Live connection state, owned by `use_sync_client`.
    pub status: Signal<SyncStatus>,
    /// URL the running client connected with at startup. Compared against
    /// `url` so a saved-but-not-restarted edit reads as "restart to connect".
    pub started_url: String,
    pub on_url_change: EventHandler<String>,
}

#[component]
pub fn SyncCard(props: SyncCardProps) -> Element {
    // Shown iff a URL is configured. Turning the toggle off clears `url`
    // outright (off means unconfigured, not paused); turning it on just
    // reveals an empty field, since there is no default server to propose.
    // `show_field` is only the toggle's intent — visibility also derives
    // from the live prop, so a URL set anywhere but here still reveals the
    // field instead of going stale behind the first render's snapshot.
    let mut show_field = use_signal(|| !props.url.trim().is_empty());
    let field_visible = *show_field.read() || !props.url.trim().is_empty();

    let line = describe_status(&props.url, &props.started_url, &props.status.read());

    rsx! {
        SubSection { label: "Sync".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Sync notes between two machines" }
                Toggle {
                    value: field_visible,
                    ontoggle: move |on: bool| {
                        show_field.set(on);
                        if !on {
                            props.on_url_change.call(String::new());
                        }
                    },
                }
            }

            if field_visible {
                div { class: "card-row card-row-top",
                    span { class: "card-label", "Other machine's address" }
                    input {
                        class: "input input-mono",
                        placeholder: "wss://your-desktop.tailnet.ts.net/sync",
                        value: "{props.url}",
                        onchange: move |e: Event<FormData>| {
                            props.on_url_change.call(normalize_sync_url(&e.value()));
                        },
                    }
                }
                div { class: "llm-note",
                    "This is the address of the other computer's sync server, not a website. Ask whoever set it up if you don't know it."
                }
            }

            // Stays visible while a pre-toggle connection is still running:
            // the client reads the address once at startup, so turning the
            // toggle off doesn't drop it until restart.
            if field_visible || !props.started_url.is_empty() {
                div { class: "card-row",
                    span { class: "sync-status {line.class}", "{line.headline}" }
                }
                if let Some(detail) = &line.detail {
                    div { class: "sync-note", "{detail}" }
                }
            }

            div { class: "llm-note",
                "Notes and task suggestions sync to your other machine. Images sync too, but through a separate program called Syncthing that you install and set up on both machines yourself. Your API keys and other settings stay on this one."
            }
        }
    }
}

/// Headline status plus an optional detail line shown underneath.
struct StatusLine {
    headline: String,
    detail: Option<String>,
    class: &'static str,
}

impl StatusLine {
    fn plain(headline: &str, class: &'static str) -> Self {
        Self { headline: headline.to_string(), detail: None, class }
    }
}

/// Status text for the current connection. Pure (no `Signal`) so tests can
/// call it directly. Accounts for the client reading the URL once at startup:
/// a saved edit takes effect only after restart.
fn describe_status(url: &str, started_url: &str, live: &SyncStatus) -> StatusLine {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        if started_url.is_empty() {
            // Toggle on, nothing typed yet.
            return StatusLine::plain("Off until you enter the address above.", "off");
        }
        // Toggled off but the old connection still runs until restart.
        return StatusLine::plain("Sync stays on until you restart Beamer.", "connecting");
    }
    if trimmed != started_url {
        return StatusLine::plain("Saved. Restart Beamer to connect.", "connecting");
    }
    match live {
        SyncStatus::Connected => StatusLine::plain("Connected. Your notes are syncing.", "ok"),
        SyncStatus::Disconnected { detail } => StatusLine {
            headline: "Can't reach the server right now. Retrying.".to_string(),
            detail: Some(detail.clone()),
            class: "bad",
        },
        // Here `Off` only means "no transition reported yet" (`url` matches a
        // non-empty `started_url`, so the client did spawn).
        SyncStatus::Off | SyncStatus::Connecting => StatusLine::plain("Connecting...", "connecting"),
    }
}

/// Normalize address-field input to the `wss://…/sync` shape the client needs.
/// Bare hostnames gain scheme + path; `http(s)://` maps to `ws(s)://`.
/// Unrecognised schemes are left alone.
pub fn normalize_sync_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let (scheme, rest) = if let Some(rest) = trimmed.strip_prefix("wss://") {
        ("wss", rest)
    } else if let Some(rest) = trimmed.strip_prefix("ws://") {
        ("ws", rest)
    } else if let Some(rest) = trimmed.strip_prefix("https://") {
        ("wss", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("ws", rest)
    } else if trimmed.contains("://") {
        return trimmed.to_string();
    } else {
        ("wss", trimmed)
    };
    if rest.contains('/') {
        format!("{scheme}://{rest}")
    } else {
        format!("{scheme}://{rest}/sync")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_hostname_gets_the_scheme_and_path() {
        assert_eq!(
            normalize_sync_url("callisto.taila63f23.ts.net"),
            "wss://callisto.taila63f23.ts.net/sync"
        );
    }

    #[test]
    fn a_bare_hostname_with_a_path_only_gets_the_scheme() {
        assert_eq!(
            normalize_sync_url("callisto.taila63f23.ts.net/sync"),
            "wss://callisto.taila63f23.ts.net/sync"
        );
    }

    #[test]
    fn an_already_correct_url_is_left_alone() {
        let url = "wss://callisto.taila63f23.ts.net/sync";
        assert_eq!(normalize_sync_url(url), url);
    }

    #[test]
    fn a_pasted_tailscale_serve_https_link_becomes_wss() {
        assert_eq!(
            normalize_sync_url("https://callisto.taila63f23.ts.net/sync"),
            "wss://callisto.taila63f23.ts.net/sync"
        );
    }

    #[test]
    fn an_https_link_missing_a_path_still_gets_one() {
        assert_eq!(
            normalize_sync_url("https://callisto.taila63f23.ts.net"),
            "wss://callisto.taila63f23.ts.net/sync"
        );
    }

    #[test]
    fn an_unrecognised_scheme_is_left_alone() {
        assert_eq!(normalize_sync_url("ftp://example.com"), "ftp://example.com");
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            normalize_sync_url("  callisto.taila63f23.ts.net  "),
            "wss://callisto.taila63f23.ts.net/sync"
        );
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(normalize_sync_url(""), "");
        assert_eq!(normalize_sync_url("   "), "");
    }

    #[test]
    fn off_when_the_url_is_empty() {
        let line = describe_status("", "", &SyncStatus::Off);
        assert_eq!(line.class, "off");
    }

    /// Toggling off clears `url` but the old connection runs until restart.
    #[test]
    fn turning_the_toggle_off_does_not_claim_sync_has_already_stopped() {
        let line = describe_status("", "wss://host/sync", &SyncStatus::Connected);
        assert_eq!(line.class, "connecting");
        assert!(line.headline.contains("restart"));
    }

    #[test]
    fn a_saved_address_the_client_has_not_picked_up_yet_asks_for_a_restart() {
        let line = describe_status("wss://new-host/sync", "wss://old-host/sync", &SyncStatus::Connected);
        assert_eq!(line.class, "connecting");
        assert!(line.headline.contains("Restart"));
    }

    #[test]
    fn a_first_enable_before_restart_also_asks_for_a_restart() {
        // started_url is empty because the client never started this run.
        let line = describe_status("wss://host/sync", "", &SyncStatus::Off);
        assert_eq!(line.class, "connecting");
        assert!(line.headline.contains("Restart"));
    }

    #[test]
    fn a_matching_url_with_no_transition_yet_reads_as_connecting_not_off() {
        let line = describe_status("wss://host/sync", "wss://host/sync", &SyncStatus::Off);
        assert_eq!(line.class, "connecting");
        assert_eq!(line.headline, "Connecting...");
    }

    #[test]
    fn a_live_connection_reads_as_connected() {
        let line = describe_status("wss://host/sync", "wss://host/sync", &SyncStatus::Connected);
        assert_eq!(line.class, "ok");
    }

    #[test]
    fn a_dropped_connection_shows_the_reason_as_a_secondary_detail() {
        let live = SyncStatus::Disconnected { detail: "connection refused".to_string() };
        let line = describe_status("wss://host/sync", "wss://host/sync", &live);
        assert_eq!(line.class, "bad");
        assert_eq!(line.detail.as_deref(), Some("connection refused"));
        assert!(!line.headline.contains("connection refused"), "the headline stays plain language");
    }
}
