use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct CardProps {
    title: String,
    children: Element,
}

#[component]
pub fn Card(props: CardProps) -> Element {
    rsx! {
        div { class: "card",
            div { class: "card-title", "{props.title}" }
            {props.children}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SelectProps {
    value: String,
    options: Vec<(String, String)>, // (value, label)
    onchange: EventHandler<String>,
}

#[component]
pub fn Select(props: SelectProps) -> Element {
    rsx! {
        select {
            class: "select",
            onchange: move |e: Event<FormData>| {
                props.onchange.call(e.value().to_string());
            },
            for (val, label) in &props.options {
                option { value: "{val}", selected: *val == props.value, "{label}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ToggleProps {
    value: bool,
    ontoggle: EventHandler<bool>,
}

#[component]
pub fn Toggle(props: ToggleProps) -> Element {
    let class = if props.value { "toggle active" } else { "toggle" };
    rsx! {
        div {
            class: "{class}",
            onclick: move |_| props.ontoggle.call(!props.value),
            div { class: "toggle-knob" }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct MaskedInputProps {
    value: String,
    onchange: EventHandler<String>,
}

#[component]
pub fn MaskedInput(props: MaskedInputProps) -> Element {
    let mut visible = use_signal(|| false);
    let input_type = if *visible.read() { "text" } else { "password" };

    rsx! {
        div { class: "masked-container",
            input {
                class: "input input-mono",
                r#type: "{input_type}",
                value: "{props.value}",
                oninput: move |e: Event<FormData>| {
                    props.onchange.call(e.value().to_string());
                },
            }
            button {
                class: "btn btn-small",
                onclick: move |_| {
                    let current = *visible.read();
                    visible.set(!current);
                },
                if *visible.read() { "Hide" } else { "Show" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TagChipProps {
    label: String,
    onremove: EventHandler<()>,
}

#[component]
pub fn TagChip(props: TagChipProps) -> Element {
    rsx! {
        div { class: "tag-chip",
            span { "{props.label}" }
            button {
                class: "tag-remove",
                onclick: move |_| props.onremove.call(()),
                "\u{2715}"
            }
        }
    }
}
