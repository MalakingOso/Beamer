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

/// Root component for the screen-edge glow window. Reads the glow color from
/// the shared `Config` signal so it updates live when appearance settings change.
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
