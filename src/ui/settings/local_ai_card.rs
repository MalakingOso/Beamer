//! Connection settings for the local model server, plus the model attribution
//! its licence requires.
//!
//! Dumb like every other card: plain values in, `EventHandler`s out, persisted
//! by `settings::save_config`. The one piece of state it owns is the probe
//! result, which nothing outside this card has a use for.

use dioxus::prelude::*;

use crate::llm::client::{self, ModelInfo};
use crate::llm::{MODEL_CREDIT, MIN_CONNECT_TIMEOUT_MS};
use crate::ui::components::{Card, Select, Toggle};

#[derive(Clone, PartialEq)]
enum Probe {
    Idle,
    Running,
    Ok(Vec<ModelInfo>),
    Failed(String),
}

#[derive(Props, Clone, PartialEq)]
pub struct LocalAiCardProps {
    pub enabled: bool,
    pub base_url: String,
    pub request_timeout_ms: u64,
    /// How long to wait for the connection itself to open, separate from the
    /// total request timeout above. Read into the shared HTTP client's
    /// `OnceLock` once, at process start (see `llm::client::init_http_client`),
    /// so editing it here does not take effect until the next restart. The
    /// card says so; do not remove that note without re-plumbing the client.
    pub connect_timeout_ms: u64,
    pub cleanup_model: String,
    pub extract_model: String,
    pub on_enabled_change: EventHandler<bool>,
    pub on_base_url_change: EventHandler<String>,
    pub on_connect_timeout_ms_change: EventHandler<u64>,
    pub on_cleanup_model_change: EventHandler<String>,
    pub on_extract_model_change: EventHandler<String>,
}

#[component]
pub fn LocalAiCard(props: LocalAiCardProps) -> Element {
    let mut probe = use_signal(|| Probe::Idle);

    let base_url = props.base_url.clone();
    let timeout = std::time::Duration::from_millis(props.request_timeout_ms);

    let start_probe = move |base_url: String| {
        spawn(async move {
            probe.set(Probe::Running);
            match client::probe(&base_url, timeout).await {
                Ok(models) => probe.set(Probe::Ok(models)),
                Err(msg) => probe.set(Probe::Failed(msg)),
            }
        });
    };

    // One probe when the settings page mounts, so the card is informative
    // without being asked. `use_hook` runs once per mount, never on a timer —
    // see the warning on `client::probe` for why that distinction is load-
    // bearing rather than merely tidy.
    use_hook({
        let base_url = base_url.clone();
        move || start_probe(base_url)
    });

    // Pick from what the server actually serves rather than typing a name that
    // has to match a GGUF stem exactly. Until a probe succeeds there is nothing
    // to pick from, so the fields stay free text seeded with the configured
    // value — an unreachable server must not make the setting uneditable.
    let served: Vec<String> = match &*probe.read() {
        Probe::Ok(models) => models.iter().map(|m| m.id.clone()).collect(),
        _ => Vec::new(),
    };

    rsx! {
        Card { title: "Local AI".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Enable on-device cleanup and task extraction" }
                Toggle {
                    value: props.enabled,
                    ontoggle: move |v: bool| props.on_enabled_change.call(v),
                }
            }

            div { class: "card-row",
                span { class: "card-label", "Server URL" }
                input {
                    class: "input input-mono",
                    value: "{props.base_url}",
                    onchange: move |e: Event<FormData>| {
                        props.on_base_url_change.call(e.value().to_string());
                    },
                }
            }

            div { class: "card-row",
                span { class: "card-label", "Connect timeout (ms)" }
                input {
                    class: "input input-mono",
                    r#type: "number",
                    min: "{MIN_CONNECT_TIMEOUT_MS}",
                    value: "{props.connect_timeout_ms}",
                    onchange: move |e: Event<FormData>| {
                        // The `min` attribute above is a hint a browser input
                        // can ignore or a hand-typed value can bypass; this
                        // clamp is what actually keeps `0` (and therefore a
                        // `Duration::ZERO` connect timeout that fails every
                        // request) out of a value this card can save. It does
                        // not protect a hand-edited config.toml, which is why
                        // `LlmConfig::connect_timeout()` clamps again at the
                        // point of use.
                        if let Ok(v) = e.value().parse::<u64>() {
                            props.on_connect_timeout_ms_change.call(v.max(MIN_CONNECT_TIMEOUT_MS));
                        }
                    },
                }
            }
            div { class: "llm-note",
                "Takes effect after a restart \u{2014} the connection pool is built once at startup."
            }

            div { class: "card-row",
                button {
                    class: "btn btn-secondary",
                    disabled: *probe.read() == Probe::Running,
                    onclick: {
                        let base_url = base_url.clone();
                        move |_| start_probe(base_url.clone())
                    },
                    if *probe.read() == Probe::Running { "Testing\u{2026}" } else { "Test connection" }
                }
                match &*probe.read() {
                    Probe::Ok(models) => rsx! {
                        span { class: "llm-status ok",
                            "Connected \u{2014} {models.len()} model(s)"
                        }
                    },
                    Probe::Failed(msg) => rsx! {
                        span { class: "llm-status bad", "{msg}" }
                    },
                    _ => rsx! {},
                }
            }

            match &*probe.read() {
                Probe::Ok(models) => rsx! {
                    div { class: "llm-models",
                        for m in models.iter() {
                            div { class: "llm-model-row", key: "{m.id}",
                                span { class: "llm-model-id", "{m.id}" }
                                span { class: "llm-model-state", "{m.status}" }
                            }
                        }
                    }
                },
                Probe::Failed(_) => rsx! {
                    // Say what still works. A model server that is down costs
                    // cleanup, never capture.
                    div { class: "llm-note",
                        "Notes are still captured \u{2014} they just are not cleaned up."
                    }
                },
                _ => rsx! {},
            }

            div { class: "card-row",
                span { class: "card-label", "Cleanup model" }
                if served.is_empty() {
                    input {
                        class: "input input-mono",
                        value: "{props.cleanup_model}",
                        onchange: move |e: Event<FormData>| {
                            props.on_cleanup_model_change.call(e.value().to_string());
                        },
                    }
                } else {
                    Select {
                        value: props.cleanup_model.clone(),
                        options: model_options(&served, &props.cleanup_model),
                        onchange: move |v: String| props.on_cleanup_model_change.call(v),
                    }
                }
            }

            div { class: "card-row",
                span { class: "card-label", "Extraction model" }
                if served.is_empty() {
                    input {
                        class: "input input-mono",
                        value: "{props.extract_model}",
                        onchange: move |e: Event<FormData>| {
                            props.on_extract_model_change.call(e.value().to_string());
                        },
                    }
                } else {
                    Select {
                        value: props.extract_model.clone(),
                        options: model_options(&served, &props.extract_model),
                        onchange: move |v: String| props.on_extract_model_change.call(v),
                    }
                }
            }

            div { class: "llm-credit",
                "Cleanup uses {MODEL_CREDIT}."
            }
        }
    }
}

/// Options for a model picker: what the server serves, plus whatever is
/// currently configured.
///
/// The configured value is kept even when the server does not serve it — a
/// `Select` whose value is absent from its options renders as the first entry,
/// which would silently show the user a different model from the one their
/// config names.
fn model_options(served: &[String], current: &str) -> Vec<(String, String)> {
    let mut opts: Vec<(String, String)> = served
        .iter()
        .map(|id| (id.clone(), id.clone()))
        .collect();
    if !current.is_empty() && !served.iter().any(|id| id == current) {
        opts.insert(0, (current.to_string(), format!("{current} (not served)")));
    }
    opts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_model_the_server_does_not_serve_is_still_offered() {
        let opts = model_options(&["a".into(), "b".into()], "gone");
        assert_eq!(opts[0].0, "gone", "the configured value must remain selectable");
        assert!(
            opts[0].1.contains("not served"),
            "and must say why it is odd, rather than looking like any other choice"
        );
        assert_eq!(opts.len(), 3);
    }

    #[test]
    fn a_served_model_is_not_duplicated() {
        let opts = model_options(&["a".into(), "b".into()], "a");
        assert_eq!(opts.len(), 2);
        assert_eq!(opts, vec![("a".into(), "a".into()), ("b".into(), "b".into())]);
    }

    #[test]
    fn an_unset_model_adds_no_placeholder_row() {
        let opts = model_options(&["a".into()], "");
        assert_eq!(opts.len(), 1);
    }
}
