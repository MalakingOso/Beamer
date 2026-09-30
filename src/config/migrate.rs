//! Load-time migrations of stored config values. Each returns `true` when it
//! changed something, so `Config::load` knows to re-save.

use super::default_backends;

/// Rename removed realtime backend ids (`elevenlabs`, `voxtral`) to their
/// `_batch` successors; anything else is untouched.
pub(super) fn migrate_transcription_backend(backend: &mut String) -> bool {
    let migrated = match backend.as_str() {
        "elevenlabs" => "elevenlabs_batch",
        "voxtral" => "voxtral_batch",
        _ => return false,
    };
    *backend = migrated.to_string();
    true
}

/// Normalize a stored injection backend chain against the current build.
pub(super) fn migrate_injection_backends(backends: &mut Vec<String>) -> bool {
    let mut dirty = false;

    // Drop removed backends. wtype is current (wlroots), not removed.
    const REMOVED: &[&str] = &["dotool", "enigo", "atspi"];
    let before = backends.len();
    backends.retain(|b| !REMOVED.contains(&b.as_str()));
    if backends.len() != before {
        dirty = true;
    }

    // Upgrade the legacy default to today's; custom orderings stay as-is.
    if backends.as_slice() == ["ydotool".to_string(), "clipboard".to_string()] {
        *backends = default_backends();
        dirty = true;
    }

    // Safety net: never leave the user with an empty backend chain.
    if backends.is_empty() {
        *backends = default_backends();
        dirty = true;
    }

    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn legacy_default_upgrades_to_new_default() {
        let mut backends = chain(&["ydotool", "clipboard"]);
        assert!(migrate_injection_backends(&mut backends));
        assert_eq!(backends, default_backends());
    }

    #[test]
    fn custom_chains_are_untouched() {
        // wtype included: it was once stripped as removed, and must not be.
        for kept in [chain(&["clipboard", "ydotool"]), chain(&["wtype", "clipboard"])] {
            let mut backends = kept.clone();
            assert!(!migrate_injection_backends(&mut backends));
            assert_eq!(backends, kept);
        }
    }

    #[test]
    fn dead_backends_are_stripped() {
        let mut backends = chain(&["dotool", "enigo", "clipboard"]);
        assert!(migrate_injection_backends(&mut backends));
        assert_eq!(backends, chain(&["clipboard"]));
    }

    #[test]
    fn empty_chain_falls_back_to_default() {
        let mut backends = chain(&["atspi"]);
        assert!(migrate_injection_backends(&mut backends));
        assert_eq!(backends, default_backends());
    }

    #[test]
    fn realtime_backends_migrate_to_batch_and_nothing_else_moves() {
        for (old, new) in [("elevenlabs", "elevenlabs_batch"), ("voxtral", "voxtral_batch")] {
            let mut backend = old.to_string();
            assert!(migrate_transcription_backend(&mut backend));
            assert_eq!(backend, new);
        }
        for kept in ["elevenlabs_batch", "elevenlabs_medical_batch", "voxtral_batch", "something_new"] {
            let mut backend = kept.to_string();
            assert!(!migrate_transcription_backend(&mut backend), "{kept}");
            assert_eq!(backend, kept);
        }
    }
}
