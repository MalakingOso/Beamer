//! Grouping chrome for the Settings page. `SettingsGroup` is the single
//! bordered container per functional group (Dictation, Intelligence, ...);
//! `SubSection` replaces the old per-card `Card` wrapper inside a group, so
//! what used to be nine separate bordered boxes becomes labeled sections
//! inside four.

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
