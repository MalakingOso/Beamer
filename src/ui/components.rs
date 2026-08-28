use dioxus::prelude::*;

/// The transcription languages Beamer offers, as (ISO 639-1 code, label).
///
/// Single source of truth: the Settings card and the Home page's Quick
/// Settings both render this list. They used to keep separate arrays, which
/// drifted — Settings offered nine languages and Home five, so selecting e.g.
/// Italian in Settings left Home's `<select>` with no matching `option` and it
/// silently displayed "English" while the config still said `it`.
pub static LANGUAGE_OPTIONS: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
];

/// `LANGUAGE_OPTIONS` in the owned-pair shape `Select` takes.
pub fn language_options() -> Vec<(String, String)> {
    LANGUAGE_OPTIONS
        .iter()
        .map(|(v, l)| (v.to_string(), l.to_string()))
        .collect()
}

/// Truncate `text` to at most `max_chars` characters, appending an ellipsis
/// when anything was cut.
///
/// Counts **characters, not bytes**: `&text[..n]` panics when `n` lands inside
/// a multi-byte UTF-8 sequence, and transcripts routinely contain the smart
/// punctuation the STT backends emit (em dashes, curly quotes, ellipses) as
/// well as accented and non-Latin scripts. See `sanitize_for_typing` in
/// `src/injection/mod.rs` for the injection-side handling of the same input.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().nth(max_chars).is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate_chars;

    #[test]
    fn short_text_is_returned_unchanged() {
        assert_eq!(truncate_chars("hello", 80), "hello");
    }

    #[test]
    fn exact_length_gets_no_ellipsis() {
        let text = "a".repeat(80);
        assert_eq!(truncate_chars(&text, 80), text);
    }

    #[test]
    fn one_over_gets_ellipsis() {
        let text = "a".repeat(81);
        let got = truncate_chars(&text, 80);
        assert_eq!(got, format!("{}...", "a".repeat(80)));
    }

    /// Regression: the byte-slicing version panicked with "byte index 80 is
    /// not a char boundary" whenever an em dash straddled the cut point —
    /// exactly what the STT backends emit for parenthetical asides.
    #[test]
    fn multibyte_char_across_the_cut_point_does_not_panic() {
        // 78 ASCII bytes, then an em dash occupying bytes 78..81, so byte
        // index 80 falls inside it.
        let head = "Please send the quarterly numbers over to accounting before the end of the day";
        let text = format!("{} \u{2014} thanks", head);
        assert!(!text.is_char_boundary(80), "test fixture must straddle byte 80");
        let got = truncate_chars(&text, 80);
        assert_eq!(got.chars().count(), 83, "80 chars plus the ellipsis");
        assert!(got.ends_with("..."));
    }

    #[test]
    fn counts_characters_not_bytes() {
        // 90 em dashes = 270 bytes but only 90 chars, so 80 chars survive.
        let text = "\u{2014}".repeat(90);
        let got = truncate_chars(&text, 80);
        assert_eq!(got, format!("{}...", "\u{2014}".repeat(80)));
    }

    #[test]
    fn non_latin_scripts_are_cut_on_character_boundaries() {
        let text = "你好世界".repeat(30); // 120 chars, 360 bytes
        let got = truncate_chars(&text, 80);
        assert_eq!(got.chars().count(), 83);
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(truncate_chars("", 80), "");
    }
}

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
                // `onchange`, not `oninput`: this fires once, when the field
                // loses focus, not on every keystroke. Callers persist the
                // value straight through (see `ApiKeysCard`'s use in the
                // Settings page), and a credential store is not something to
                // write to on every character typed.
                onchange: move |e: Event<FormData>| {
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
