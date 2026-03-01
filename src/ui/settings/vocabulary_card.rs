use dioxus::prelude::*;

use crate::ui::components::{Card, TagChip};

#[derive(Props, Clone, PartialEq)]
pub struct VocabularyCardProps {
    terms: Vec<String>,
    on_add: EventHandler<String>,
    on_remove: EventHandler<String>,
}

#[component]
pub fn VocabularyCard(props: VocabularyCardProps) -> Element {
    let mut new_term = use_signal(|| String::new());

    rsx! {
        Card { title: "Vocabulary".to_string(),
            div { class: "tag-list",
                for term in &props.terms {
                    TagChip {
                        key: "{term}",
                        label: term.clone(),
                        onremove: {
                            let term = term.clone();
                            move |_| props.on_remove.call(term.clone())
                        },
                    }
                }
            }
            div { class: "card-row",
                input {
                    class: "input input-mono",
                    placeholder: "Add term...",
                    value: "{new_term}",
                    oninput: move |e: Event<FormData>| {
                        new_term.set(e.value().to_string());
                    },
                    onkeypress: move |e: Event<KeyboardData>| {
                        if e.key() == Key::Enter {
                            let term = new_term.read().clone();
                            if !term.trim().is_empty() {
                                props.on_add.call(term);
                                new_term.set(String::new());
                            }
                        }
                    },
                }
                button {
                    class: "btn btn-small",
                    onclick: move |_| {
                        let term = new_term.read().clone();
                        if !term.trim().is_empty() {
                            props.on_add.call(term);
                            new_term.set(String::new());
                        }
                    },
                    "+ Add"
                }
            }
        }
    }
}
