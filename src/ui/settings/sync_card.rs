//! Settings card for the live sync client. Aimed at someone who has never
//! heard of a CRDT or a WebSocket: a toggle, one address field, and a plain
//! sentence describing what the connection is actually doing right now.
//!
//! Dumb like every other card: plain values in, `EventHandler`s out,
//! persisted by `settings::save_config`, except `status`, which is the
//! `Signal<sync_client::SyncStatus>` `use_sync_client` already writes.
//! `SyncCard` reads it; it never writes it, and it never opens a socket of
//! its own to check. See `sync_client.rs` for why that would mean a second
//! `sync::State` and a second peer from the server's point of view.

use dioxus::prelude::*;

use crate::notes::sync_client::SyncStatus;
use crate::ui::components::{Card, Toggle};

#[derive(Props, Clone, PartialEq)]
pub struct SyncCardProps {
    /// Empty means sync is off. The same convention `note_hotkey` uses.
    pub url: String,
    /// The live connection's state, owned and written by `use_sync_client`.
    pub status: Signal<SyncStatus>,
    /// The URL the running client actually connected with, fixed at
    /// startup. Compared against `url` so an edit that has been saved but
    /// not yet applied (Beamer has not restarted) reads as "saved, restart
    /// to connect" instead of describing the old connection as the new one.
    pub started_url: String,
    pub on_url_change: EventHandler<String>,
}

#[component]
pub fn SyncCard(props: SyncCardProps) -> Element {
    // Whether the address field is shown at all. Seeded from whether a URL
    // is already configured, but tracked separately from it: turning the
    // toggle on has no default server to propose (unlike note capture's
    // chord), so it can only reveal an empty field for the user to fill in,
    // not write a value that would make `url` non-empty by itself. Turning
    // it off clears the URL outright, matching `note_hotkey`'s precedent
    // that off must genuinely mean unconfigured, not remembered-but-paused.
    let mut show_field = use_signal(|| !props.url.trim().is_empty());

    let line = describe_status(&props.url, &props.started_url, &props.status.read());

    rsx! {
        Card { title: "Sync".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Sync notes between two machines" }
                Toggle {
                    value: *show_field.read(),
                    ontoggle: move |on: bool| {
                        show_field.set(on);
                        if !on {
                            props.on_url_change.call(String::new());
                        }
                    },
                }
            }

            if *show_field.read() {
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

            // Shown even after the field above is hidden by turning the
            // toggle off: `use_sync_client` reads the address once, at
            // startup, and does not tear the connection down on an edit (see
            // its doc). A running connection outliving the toggle that just
            // asked to turn it off is exactly the gap this status line exists
            // to close, so it stays visible until the connection genuinely
            // has nothing left to report.
            if *show_field.read() || !props.started_url.is_empty() {
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

/// One line of plain-language status, plus an optional technical detail the
/// card shows underneath it in a smaller, quieter line.
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

/// What the card should say about the connection right now.
///
/// Combines the live socket state with whether a saved address has actually
/// taken effect: `use_sync_client` reads `config.sync.url` once, at mount,
/// so an edit made through this card cannot reach the running client until
/// the next restart (see that module's doc). Without this check the status
/// line would keep describing the *old* address's connection, or lack of
/// one, as though it were a verdict on the address just typed in.
///
/// Pure and independent of any `Signal`, so it is testable directly rather
/// than through a Dioxus render.
fn describe_status(url: &str, started_url: &str, live: &SyncStatus) -> StatusLine {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        if started_url.is_empty() {
            // The only way to reach this line is the toggle being on with
            // nothing typed yet: the card hides this whole status block once
            // the toggle is off, unless a connection from before is still
            // running, which is the branch right below.
            return StatusLine::plain("Off until you enter the address above.", "off");
        }
        // The toggle was switched off, which cleared `url`, but the
        // connection `started_url` names is still running: `use_sync_client`
        // only reads `config.sync.url` once, at startup, and does not tear a
        // connection down when the config it started from changes. Saying
        // nothing here would leave the card claiming sync is off while notes
        // keep leaving this machine.
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
        // `SyncStatus::Off` can only mean "the client has not reported its
        // first transition yet" here, since `trimmed == started_url` and
        // it is non-empty, which is exactly the condition `use_sync_client`
        // checks before spawning the client at all.
        SyncStatus::Off | SyncStatus::Connecting => StatusLine::plain("Connecting...", "connecting"),
    }
}

/// Turn what a user actually types into the address field into the
/// `wss://…` shape the client needs.
///
/// Someone typing a Tailscale hostname straight off their machine list, or
/// pasting the `https://` link `tailscale serve` prints, should not have to
/// know that a sync connection wants `wss://` and a `/sync` path. Anything
/// that already names a scheme this function does not recognise is left
/// exactly alone rather than guessed at.
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

    /// The toggle was just switched off, clearing `url`, but the connection
    /// it started with is still running until a restart. The card must keep
    /// saying so rather than going quiet as though sync had already stopped.
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
