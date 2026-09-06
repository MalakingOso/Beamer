//! Tests for [`super`]: `HotkeyEvent`/`CaptureMode`, `HotkeyConfig::parse`,
//! and the platform-neutral binding-matching layer shared by
//! `linux_hotkey.rs` and `ll_hook.rs`.
//!
//! Split into its own file per the project's 500-line cap. `mod.rs` holds
//! two backends' worth of shared matching logic plus its own config parsing,
//! and the test suite for all of that does not fit alongside it.

use super::*;

#[test]
fn record_start_carries_its_capture_mode() {
    let inject = HotkeyEvent::RecordStart(CaptureMode::Inject);
    let note = HotkeyEvent::RecordStart(CaptureMode::Note);

    assert_ne!(
        inject, note,
        "the orchestrator must be able to tell the two hotkeys apart"
    );
    match note {
        HotkeyEvent::RecordStart(mode) => assert_eq!(mode, CaptureMode::Note),
        HotkeyEvent::RecordStop => panic!("wrong variant"),
    }
}

#[test]
fn capture_mode_defaults_to_inject() {
    assert_eq!(
        CaptureMode::default(),
        CaptureMode::Inject,
        "an unconfigured note hotkey must never silently divert dictation"
    );
}

#[test]
fn super_with_another_key_is_rejected() {
    assert!(
        HotkeyConfig::parse("Super+N", false).is_none(),
        "Super paired with another key has no Meta field to carry it, so the \
         parse must fail rather than silently drop Super and fire on bare N"
    );
    assert!(HotkeyConfig::parse("Ctrl+Super+N", false).is_none());
}

#[test]
fn a_chord_with_no_modifier_at_all_is_rejected() {
    for chord in ["", "N", "Space", "F9", "Enter"] {
        assert!(
            HotkeyConfig::parse(chord, false).is_none(),
            "{chord:?} would fire on every unmodified keypress; that is never intent"
        );
    }
}

#[test]
fn super_alone_still_parses_as_the_trigger() {
    let cfg = HotkeyConfig::parse("Ctrl+Super", false).expect("Super alone must still parse");
    assert!(cfg.ctrl);
    assert_eq!(cfg.trigger_vk, VK_LWIN);
}

fn cfg(ctrl: bool, shift: bool, vk: u32, toggle: bool) -> HotkeyConfig {
    HotkeyConfig { ctrl, alt: false, shift, trigger_vk: vk, is_toggle: toggle }
}

/// Ctrl+Space -> inject (hold), Ctrl+Shift+N -> note (toggle).
fn two_bindings() -> Vec<BindingConfig> {
    vec![
        BindingConfig { mode: CaptureMode::Inject, config: cfg(true, false, 0x20, false) },
        BindingConfig { mode: CaptureMode::Note,   config: cfg(true, true,  0x4E, true) },
    ]
}

/// Two bindings that share a trigger key and differ only by a modifier.
///
/// This is the real-world pairing: `Ctrl+Super` dictates, `Ctrl+Alt+Super`
/// captures a note. Both resolve to `trigger_vk == VK_LWIN`, because the
/// parser can only express Super as a trigger, and `HotkeyConfig` has no
/// Super/Meta modifier field at all. Nothing separates them except the
/// exact modifier comparison in `matching_binding`, so it is worth pinning:
/// a future `mods.ctrl >= b.config.ctrl`-style relaxation would make every
/// note chord also fire dictation.
#[test]
fn chords_sharing_a_trigger_key_are_told_apart_by_modifiers_alone() {
    let bindings = vec![
        BindingConfig {
            mode: CaptureMode::Inject,
            config: HotkeyConfig { ctrl: true, alt: false, shift: false, trigger_vk: VK_LWIN, is_toggle: false },
        },
        BindingConfig {
            mode: CaptureMode::Note,
            config: HotkeyConfig { ctrl: true, alt: true, shift: false, trigger_vk: VK_LWIN, is_toggle: true },
        },
    ];
    let mods = |ctrl, alt| Modifiers { ctrl, alt, shift: false };

    assert_eq!(matching_binding(&bindings, VK_LWIN, mods(true, false)), Some(0));
    assert_eq!(matching_binding(&bindings, VK_LWIN, mods(true, true)), Some(1));
    assert_eq!(
        matching_binding(&bindings, VK_LWIN, mods(false, true)),
        None,
        "a chord neither binding asked for must fire neither"
    );
}

#[test]
fn each_binding_matches_only_its_own_chord() {
    let bindings = two_bindings();

    // Ctrl held, Shift not: Space matches inject, N matches nothing.
    let mods = Modifiers { ctrl: true, alt: false, shift: false };
    assert_eq!(matching_binding(&bindings, 0x20, mods), Some(0));
    assert_eq!(matching_binding(&bindings, 0x4E, mods), None);

    // Ctrl+Shift held: N matches note, Space matches nothing.
    let mods = Modifiers { ctrl: true, alt: false, shift: true };
    assert_eq!(matching_binding(&bindings, 0x4E, mods), Some(1));
    assert_eq!(
        matching_binding(&bindings, 0x20, mods), None,
        "Ctrl+Shift+Space must not trigger the plain Ctrl+Space binding"
    );
}

#[test]
fn bindings_keep_independent_press_state() {
    let mut state = [BindingState::default(); MAX_BINDINGS];

    state[0].trigger_held = true;
    state[0].armed = true;

    assert!(!state[1].trigger_held, "note binding must not inherit inject's held state");
    assert!(!state[1].armed, "note binding must not inherit inject's armed state");
}

#[test]
fn absent_note_binding_yields_only_one_binding() {
    let bindings = build_bindings(cfg(true, false, 0x20, false), None);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].mode, CaptureMode::Inject);
}
