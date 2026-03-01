use dioxus::prelude::*;

use crate::config::Config;

#[derive(Props, Clone, PartialEq)]
pub struct GlowProps {
    color: String,
}

#[component]
pub fn Glow(props: GlowProps) -> Element {
    rsx! {
        div {
            class: "glow",
            style: "--glow-color: {props.color};",
        }
    }
}

/// Standalone app component for the glow window. Reads color from context.
#[component]
pub fn GlowApp() -> Element {
    let config: Signal<Config> = use_context();
    let color = config.read().appearance.glow_color.clone();

    rsx! {
        head {
            link { rel: "stylesheet", href: asset!("assets/styles.css") }
        }
        div {
            class: "glow",
            style: "--glow-color: {color};",
        }
    }
}
