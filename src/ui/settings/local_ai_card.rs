//! Local model server connection settings. Owns only the probe result.

use dioxus::prelude::*;

use crate::llm::client::{self, ModelInfo};
use crate::llm::{MODEL_CREDIT, MIN_CONNECT_TIMEOUT_MS};
use crate::model_setup::{self, DownloadStatus};
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
    /// Connect timeout, separate from the request timeout. Baked into the HTTP
    /// client at startup, so edits take effect after a restart (see the note).
    pub connect_timeout_ms: u64,
    /// Transcript cleanup, toggled on its own: it needs a second model the
    /// installer never fetches, so it is off unless the user asks for it.
    pub cleanup_enabled: bool,
    pub cleanup_model: String,
    pub extract_model: String,
    pub on_enabled_change: EventHandler<bool>,
    pub on_base_url_change: EventHandler<String>,
    pub on_connect_timeout_ms_change: EventHandler<u64>,
    pub on_cleanup_enabled_change: EventHandler<bool>,
    pub on_cleanup_model_change: EventHandler<String>,
    pub on_extract_model_change: EventHandler<String>,
    /// K2-Horizon's first-run download/setup state — `Idle` except on a
    /// fresh, bundled aarch64 install. See `crate::model_setup`.
    pub download_status: Signal<crate::model_setup::DownloadStatus>,
}

#[component]
pub fn LocalAiCard(props: LocalAiCardProps) -> Element {
    let mut probe = use_signal(|| Probe::Idle);

    let base_url = props.base_url.clone();
    let timeout = std::time::Duration::from_millis(props.request_timeout_ms);
    let download_status = props.download_status;

    let start_probe = move |base_url: String| {
        spawn(async move {
            probe.set(Probe::Running);
            match client::probe(&base_url, timeout).await {
                Ok(models) => probe.set(Probe::Ok(models)),
                Err(msg) => probe.set(Probe::Failed(msg)),
            }
        });
    };

    // One probe per mount so the card shows live data without being asked.
    use_hook({
        let base_url = base_url.clone();
        move || start_probe(base_url)
    });

    // Pick from served models once a probe succeeds; free text until then so
    // an unreachable server never makes the setting uneditable.
    let served: Vec<String> = match &*probe.read() {
        Probe::Ok(models) => models.iter().map(|m| m.id.clone()).collect(),
        _ => Vec::new(),
    };

    rsx! {
        Card { title: "Local AI".to_string(),
            div { class: "card-row",
                span { class: "card-label", "Enable on-device AI" }
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
                        // `min` is bypassable; this clamp is what actually keeps
                        // a `0` out of the saved value. Hand-edited config.toml
                        // is clamped again by `LlmConfig::connect_timeout()`.
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
                    // A down server costs the model passes, never capture.
                    div { class: "llm-note",
                        "Notes are still captured \u{2014} tasks just are not found in them."
                    }
                },
                _ => rsx! {},
            }

            div { class: "card-row",
                span { class: "card-label", "Clean up dictated transcripts" }
                Toggle {
                    value: props.cleanup_enabled,
                    ontoggle: move |v: bool| props.on_cleanup_enabled_change.call(v),
                }
            }
            div { class: "llm-note",
                "Off by default \u{2014} it needs a second model this install does not fetch. \
                 Task extraction is separate, and unaffected by this."
            }

            if props.cleanup_enabled {
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

            match &*download_status.read() {
                DownloadStatus::Idle | DownloadStatus::Ready => rsx! {},
                DownloadStatus::Verifying => rsx! {
                    div { class: "card-row",
                        span { class: "llm-status", "Verifying K2-Horizon model\u{2026}" }
                    }
                },
                DownloadStatus::Downloading { bytes, total } => {
                    let pct = if *total > 0 {
                        (*bytes as f64 / *total as f64 * 100.0) as u32
                    } else {
                        0
                    };
                    rsx! {
                        div { class: "card-row",
                            span { class: "llm-status", "Downloading K2-Horizon model\u{2026} {pct}%" }
                            button {
                                class: "btn btn-secondary",
                                onclick: move |_| model_setup::cancel(download_status),
                                "Cancel"
                            }
                        }
                    }
                },
                DownloadStatus::Failed(msg) => rsx! {
                    div { class: "card-row",
                        span { class: "llm-status bad", "K2-Horizon setup failed: {msg}" }
                        button {
                            class: "btn btn-secondary",
                            onclick: move |_| model_setup::retry(download_status),
                            "Retry download"
                        }
                    }
                },
            }

            // The licence binds the credit to the model actually being used;
            // with cleanup off, s1-mini is never loaded.
            if props.cleanup_enabled {
                div { class: "llm-credit",
                    "Cleanup uses {MODEL_CREDIT}."
                }
            }
        }
    }
}

/// Picker options: served models plus the configured value, which is kept even
/// when unserved (a `Select` missing its value renders the first entry,
/// silently showing the wrong model).
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
        assert_eq!(opts[0].0, "gone");
        assert!(opts[0].1.contains("not served"));
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
