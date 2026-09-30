//! Tests for [`super`]: `HotkeyEvent`/`CaptureMode`, `HotkeyConfig::parse`,
//! the shared binding-matching layer, and (Linux) how the GNOME desktop grab
//! and evdev share one state machine. Split out of `mod.rs` for the 500-line cap.

use super::*;

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

/// Two bindings sharing a trigger, differing only by a modifier: the real-world
/// `Ctrl+Super` (dictate) / `Ctrl+Alt+Super` (note) pair, both `VK_LWIN`.
/// Pins exact modifier matching: a `mods.ctrl >= b.config.ctrl`-style relaxation
/// would make every note chord also fire dictation.
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

/// The GNOME desktop grab: accelerator strings and how its events share the
/// evdev state machine. Linux-only, like both modules it exercises.
#[cfg(target_os = "linux")]
mod desktop_grab {
    use super::*;
    use crate::hotkey::gnome_grab::hotkey_to_accelerator;
    use crate::hotkey::linux_hotkey::{
        handle_key_event, on_grab_event, release_desktop_bindings, HookState,
    };
    use evdev::KeyCode;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn accel(chord: &str) -> Option<String> {
        hotkey_to_accelerator(&HotkeyConfig::parse(chord, false).expect("chord parses"))
    }

    #[test]
    fn modifiers_come_out_in_one_fixed_order() {
        // Input order must not matter; Mutter's parser doesn't care, but
        // the worker's "unchanged, skip the regrab" check compares strings.
        assert_eq!(accel("Shift+Alt+Ctrl+Space").as_deref(), Some("<Control><Alt><Shift>space"));
        assert_eq!(accel("Ctrl+Shift+Space").as_deref(), Some("<Control><Shift>space"));
    }

    #[test]
    fn keys_map_to_xkb_keysym_names() {
        assert_eq!(accel("Ctrl+Shift+N").as_deref(), Some("<Control><Shift>n"));
        assert_eq!(accel("Alt+7").as_deref(), Some("<Alt>7"));
        assert_eq!(accel("Ctrl+F12").as_deref(), Some("<Control>F12"));
        assert_eq!(accel("Ctrl+PageUp").as_deref(), Some("<Control>Page_Up"));
        assert_eq!(accel("Ctrl+Backspace").as_deref(), Some("<Control>BackSpace"));
        assert_eq!(accel("Ctrl+Enter").as_deref(), Some("<Control>Return"));
    }

    #[test]
    fn a_super_trigger_is_never_grabbed() {
        // Collides with Mutter's overlay key; it stays on evdev.
        assert_eq!(accel("Ctrl+Super"), None);
        assert_eq!(accel("Ctrl+Alt+Super"), None);
    }

    /// Inject on Ctrl+Space (hold, or toggle), no note binding.
    fn state(toggle: bool) -> (HookState, Arc<[AtomicBool; MAX_BINDINGS]>, UnboundedReceiver<HotkeyEvent>) {
        let (tx, rx) = unbounded_channel();
        let bindings = build_bindings(cfg(true, false, 0x20, toggle), None);
        let owned: Arc<[AtomicBool; MAX_BINDINGS]> = Default::default();
        let state = HookState::new(
            Arc::new(Mutex::new(bindings)),
            Arc::new(AtomicBool::new(false)),
            owned.clone(),
            tx,
        );
        (state, owned, rx)
    }

    fn drain(rx: &mut UnboundedReceiver<HotkeyEvent>) -> Vec<HotkeyEvent> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn evdev_chord_press(state: &mut HookState) {
        handle_key_event(KeyCode::KEY_LEFTCTRL, 1, state);
        handle_key_event(KeyCode::KEY_SPACE, 1, state);
    }

    #[test]
    fn evdev_ignores_a_press_the_desktop_grab_owns() {
        let (mut state, owned, mut rx) = state(false);
        owned[0].store(true, Ordering::Relaxed);
        evdev_chord_press(&mut state);
        assert!(drain(&mut rx).is_empty(), "the grab reports this press itself");

        // Same chord, grab gone: evdev is back in charge.
        owned[0].store(false, Ordering::Relaxed);
        handle_key_event(KeyCode::KEY_SPACE, 0, &mut state);
        handle_key_event(KeyCode::KEY_SPACE, 1, &mut state);
        assert_eq!(drain(&mut rx), vec![HotkeyEvent::RecordStart(CaptureMode::Inject)]);
    }

    #[test]
    fn a_repeated_grab_press_starts_one_recording() {
        let (mut state, owned, mut rx) = state(false);
        owned[0].store(true, Ordering::Relaxed);
        on_grab_event(0, true, &mut state);
        on_grab_event(0, true, &mut state);
        on_grab_event(0, false, &mut state);
        assert_eq!(
            drain(&mut rx),
            vec![HotkeyEvent::RecordStart(CaptureMode::Inject), HotkeyEvent::RecordStop]
        );
    }

    #[test]
    fn a_toggle_press_release_pair_flips_once() {
        let (mut state, owned, mut rx) = state(true);
        owned[0].store(true, Ordering::Relaxed);
        on_grab_event(0, true, &mut state);
        on_grab_event(0, false, &mut state);
        assert_eq!(drain(&mut rx), vec![HotkeyEvent::RecordStart(CaptureMode::Inject)]);
        on_grab_event(0, true, &mut state);
        on_grab_event(0, false, &mut state);
        assert_eq!(drain(&mut rx), vec![HotkeyEvent::RecordStop]);
    }

    #[test]
    fn an_evdev_release_ends_a_hold_the_grab_started() {
        // Local safety net should the grab's own release ever be lost.
        let (mut state, owned, mut rx) = state(false);
        owned[0].store(true, Ordering::Relaxed);
        evdev_chord_press(&mut state);
        on_grab_event(0, true, &mut state);
        handle_key_event(KeyCode::KEY_SPACE, 0, &mut state);
        on_grab_event(0, false, &mut state);
        assert_eq!(
            drain(&mut rx),
            vec![HotkeyEvent::RecordStart(CaptureMode::Inject), HotkeyEvent::RecordStop]
        );
    }

    #[test]
    fn a_config_edit_ends_a_grab_recording_without_any_evdev_input() {
        // Remoted in over RDP, evdev never sees a key to apply the reset on.
        let (tx, mut rx) = unbounded_channel();
        let reset = Arc::new(AtomicBool::new(false));
        let owned: Arc<[AtomicBool; MAX_BINDINGS]> = Default::default();
        owned[0].store(true, Ordering::Relaxed);
        let mut state = HookState::new(
            Arc::new(Mutex::new(build_bindings(cfg(true, false, 0x20, true), None))),
            reset.clone(),
            owned,
            tx,
        );
        on_grab_event(0, true, &mut state);
        on_grab_event(0, false, &mut state);
        drain(&mut rx);
        reset.store(true, Ordering::Relaxed);
        on_grab_event(0, true, &mut state);
        assert_eq!(
            drain(&mut rx),
            vec![HotkeyEvent::RecordStop, HotkeyEvent::RecordStart(CaptureMode::Inject)],
            "the latched toggle ends before the new press starts afresh"
        );
    }

    #[test]
    fn losing_the_grab_ends_a_hold_but_keeps_a_toggle() {
        let (mut state, owned, mut rx) = state(false);
        owned[0].store(true, Ordering::Relaxed);
        on_grab_event(0, true, &mut state);
        drain(&mut rx);
        release_desktop_bindings(&mut state);
        assert_eq!(drain(&mut rx), vec![HotkeyEvent::RecordStop]);
        assert!(!owned[0].load(Ordering::Relaxed), "evdev takes the binding back");

        let (mut state, owned, mut rx) = self::state(true);
        owned[0].store(true, Ordering::Relaxed);
        on_grab_event(0, true, &mut state);
        drain(&mut rx);
        release_desktop_bindings(&mut state);
        assert!(drain(&mut rx).is_empty(), "a screen lock mid-note must not end the note");
    }
}
