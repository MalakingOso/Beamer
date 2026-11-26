//! What the tray menu's live items say. Pure functions of app state so the
//! wording is unit-tested; `ui::app_menu` applies them to the menu items.
//! No `&` in any label: muda reads it as a mnemonic marker.

use super::glyphs::Glyph;
use crate::orchestrator::RecordingState;
use crate::update::UpdateStatus;

/// The `last_injection` signal's placeholder before anything has been dictated.
pub const NO_INJECTION_YET: &str = "No injection yet";

/// Header line: the running version and what Beamer is doing right now.
pub fn status_text(state: RecordingState) -> String {
    let doing = match state {
        RecordingState::Idle => "Ready",
        RecordingState::Recording => "Recording…",
        RecordingState::Processing => "Transcribing…",
    };
    format!("Beamer v{} · {doing}", env!("CARGO_PKG_VERSION"))
}

/// Header subtitle: how to start dictating, from the configured hotkey/mode.
pub fn hotkey_hint(hotkey: &str, mode: &str) -> String {
    let hotkey = hotkey.trim();
    if hotkey.is_empty() {
        return "No dictation hotkey set".into();
    }
    if mode == "toggle" {
        format!("Press {hotkey} to start or stop")
    } else {
        format!("Hold {hotkey} to dictate")
    }
}

/// Text, glyph and enabled state for the update item.
pub fn update_item(status: &UpdateStatus) -> (String, Glyph, bool) {
    match status {
        UpdateStatus::Idle | UpdateStatus::Error(_) => ("Check for Updates".into(), Glyph::Update, true),
        UpdateStatus::Checking => ("Checking for Updates…".into(), Glyph::Update, false),
        UpdateStatus::Available { version } => (
            format!("Update to v{}", version.trim_start_matches('v')),
            Glyph::UpdateReady,
            true,
        ),
        UpdateStatus::Downloading => ("Downloading Update…".into(), Glyph::Update, false),
        UpdateStatus::ReadyToRestart => ("Restart to Update".into(), Glyph::UpdateReady, true),
    }
}

/// Whether a click on the update item should open Settings without starting
/// another check: a found or downloaded update is installed from there.
pub fn update_click_opens_only(status: &UpdateStatus) -> bool {
    matches!(status, UpdateStatus::Available { .. } | UpdateStatus::ReadyToRestart)
}

/// Text and glyph for the notes toggle, given how many note windows are open;
/// the label names what a click does.
pub fn notes_item(open_count: usize) -> (String, Glyph) {
    if open_count > 0 {
        (format!("Hide All Notes ({open_count})"), Glyph::NotesHide)
    } else {
        ("Show All Notes".into(), Glyph::NotesShow)
    }
}

/// Whether there is a transcript for "Paste Last Transcript" to paste.
pub fn has_last_injection(last_injection: &str) -> bool {
    !last_injection.is_empty() && last_injection != NO_INJECTION_YET
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_follows_recording_state() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(status_text(RecordingState::Idle), format!("Beamer v{v} · Ready"));
        assert_eq!(status_text(RecordingState::Recording), format!("Beamer v{v} · Recording…"));
        assert_eq!(status_text(RecordingState::Processing), format!("Beamer v{v} · Transcribing…"));
    }

    #[test]
    fn hint_reflects_mode_and_blank_hotkey() {
        assert_eq!(hotkey_hint("Ctrl+Space", "hold"), "Hold Ctrl+Space to dictate");
        assert_eq!(hotkey_hint("Ctrl+Space", "toggle"), "Press Ctrl+Space to start or stop");
        assert_eq!(hotkey_hint("  ", "hold"), "No dictation hotkey set");
    }

    #[test]
    fn update_item_tracks_status() {
        let (text, _, enabled) = update_item(&UpdateStatus::Idle);
        assert_eq!((text.as_str(), enabled), ("Check for Updates", true));
        let (_, _, enabled) = update_item(&UpdateStatus::Checking);
        assert!(!enabled);
        let (text, glyph, enabled) = update_item(&UpdateStatus::Available { version: "v1.0.6".into() });
        assert_eq!((text.as_str(), glyph, enabled), ("Update to v1.0.6", Glyph::UpdateReady, true));
        let (text, _, _) = update_item(&UpdateStatus::Available { version: "1.0.6".into() });
        assert_eq!(text, "Update to v1.0.6");
        let (_, _, enabled) = update_item(&UpdateStatus::Downloading);
        assert!(!enabled);
    }

    #[test]
    fn only_a_found_update_skips_the_recheck() {
        assert!(update_click_opens_only(&UpdateStatus::Available { version: "1".into() }));
        assert!(update_click_opens_only(&UpdateStatus::ReadyToRestart));
        assert!(!update_click_opens_only(&UpdateStatus::Idle));
        assert!(!update_click_opens_only(&UpdateStatus::Error("x".into())));
    }

    #[test]
    fn notes_label_names_the_click_action() {
        assert_eq!(notes_item(3), ("Hide All Notes (3)".to_string(), Glyph::NotesHide));
        assert_eq!(notes_item(0), ("Show All Notes".to_string(), Glyph::NotesShow));
    }

    #[test]
    fn paste_needs_a_real_transcript() {
        assert!(!has_last_injection(NO_INJECTION_YET));
        assert!(!has_last_injection(""));
        assert!(has_last_injection("hello"));
    }

    #[test]
    fn no_label_contains_a_mnemonic_ampersand() {
        for state in [RecordingState::Idle, RecordingState::Recording, RecordingState::Processing] {
            assert!(!status_text(state).contains('&'));
        }
        assert!(!hotkey_hint("Ctrl+Space", "hold").contains('&'));
    }
}
