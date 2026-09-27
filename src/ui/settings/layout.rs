//! Grouping chrome for the Settings page. `SettingsGroup` is the one bordered
//! container per functional group (Dictation, Intelligence, ...); each card
//! inside renders as a labeled `SubSection` rather than its own `Card`.

use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct SettingsGroupProps {
    pub title: String,
    pub children: Element,
}

#[component]
pub fn SettingsGroup(props: SettingsGroupProps) -> Element {
    rsx! {
        div { class: "settings-group",
            div { class: "settings-group-title", "{props.title}" }
            div { class: "settings-group-body", {props.children} }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SubSectionProps {
    pub label: String,
    pub children: Element,
}

#[component]
pub fn SubSection(props: SubSectionProps) -> Element {
    rsx! {
        div { class: "settings-subsection",
            div { class: "settings-subsection-label", "{props.label}" }
            {props.children}
        }
    }
}
