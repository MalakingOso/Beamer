/// Which sink a recording session's final transcript should reach.
///
/// Beamer has two dictation hotkeys that share the entire audio and ASR
/// pipeline and differ only in what happens to the finished text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureMode {
    /// Inject into the focused input field — the original behaviour.
    #[default]
    Inject,
    /// Create a sticky note.
    Note,
}

/// Sent from the hotkey listener to the orchestrator coroutine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    RecordStart(CaptureMode),
    RecordStop,
}

// Virtual key constants (Windows VK codes used as portable key IDs)
pub const VK_LWIN: u32 = 0x5B;

/// Cross-platform hotkey configuration.
/// Stores modifier flags and a trigger key as a Windows VK code (used on all platforms
/// as a stable, portable integer key identifier in the config and key-name tables).
#[derive(Clone)]
pub struct HotkeyConfig {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub trigger_vk: u32,
    pub is_toggle: bool,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            trigger_vk: 0x20, // VK_SPACE
            is_toggle: false,
        }
    }
}

impl HotkeyConfig {
    pub fn parse(hotkey_str: &str, is_toggle: bool) -> Option<Self> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut has_win = false;
        let mut key_str = String::new();

        for part in hotkey_str.split('+') {
            match part.trim().to_uppercase().as_str() {
                "CTRL" | "CONTROL" => ctrl = true,
                "ALT" | "OPTION" => alt = true,
                "SHIFT" => shift = true,
                "SUPER" | "WIN" | "CMD" | "COMMAND" | "META" => has_win = true,
                other => key_str = other.to_string(),
            }
        }

        // Super/Win/Cmd/Meta alone is representable as VK_LWIN, but paired
        // with another key it is not: `HotkeyConfig` has no Meta modifier
        // field, so "Super+N" would otherwise parse as a bare N trigger and
        // fire on every N keypress. Reject rather than silently drop it,
        // since callers already treat `None` as unbound.
        let trigger_vk = if has_win {
            if key_str.is_empty() {
                VK_LWIN
            } else {
                return None;
            }
        } else if !key_str.is_empty() {
            key_name_to_vk(&key_str)?
        } else {
            0x20 // VK_SPACE default
        };

        Some(Self {
            ctrl,
            alt,
            shift,
            trigger_vk,
            is_toggle,
        })
    }
}

pub fn key_name_to_vk(name: &str) -> Option<u32> {
    match name.to_uppercase().as_str() {
        "SPACE" => Some(0x20),
        "ENTER" | "RETURN" => Some(0x0D),
        "TAB" => Some(0x09),
        "BACKSPACE" | "BACK" => Some(0x08),
        "DELETE" => Some(0x2E),
        "INSERT" => Some(0x2D),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" => Some(0x21),
        "PAGEDOWN" => Some(0x22),
        "UP" => Some(0x26),
        "DOWN" => Some(0x28),
        "LEFT" => Some(0x25),
        "RIGHT" => Some(0x27),
        s if s.len() == 1 => {
            let c = s.as_bytes()[0];
            if c.is_ascii_uppercase() || c.is_ascii_digit() {
                Some(c as u32)
            } else {
                None
            }
        }
        s if s.starts_with('F') && s.len() <= 3 => {
            let n: u32 = s[1..].parse().ok()?;
            if (1..=12).contains(&n) {
                Some(0x70 + n - 1)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ─── Shared binding-matching layer ─────────────────────────────────────────
//
// Both platform backends (`linux_hotkey.rs`, `ll_hook.rs`) drive the same two
// hotkeys, inject and note, through this matching logic. It lives here, not
// in either backend, so a change to how bindings are compared can't drift
// between platforms.

/// Beamer has exactly two dictation hotkeys: inject and note.
pub const MAX_BINDINGS: usize = 2;

/// One configured hotkey and the sink it selects.
#[derive(Clone)]
pub struct BindingConfig {
    pub mode: CaptureMode,
    pub config: HotkeyConfig,
}

/// Per-binding press state. Kept separate from `BindingConfig` because the
/// config is swapped wholesale by `update_configs` while press state must
/// survive — a user editing the note hotkey mid-hold shouldn't strand the
/// inject binding in `armed`.
#[derive(Clone, Copy, Default)]
pub struct BindingState {
    pub armed: bool,
    pub toggled_on: bool,
    pub trigger_held: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

pub fn build_bindings(inject: HotkeyConfig, note: Option<HotkeyConfig>) -> Vec<BindingConfig> {
    let mut v = vec![BindingConfig { mode: CaptureMode::Inject, config: inject }];
    if let Some(note) = note {
        v.push(BindingConfig { mode: CaptureMode::Note, config: note });
    }
    v
}

/// Index of the binding whose trigger key and modifier set both match, or
/// `None`. Modifiers must match *exactly*, so Ctrl+Shift+Space does not fire a
/// binding registered for plain Ctrl+Space.
pub fn matching_binding(bindings: &[BindingConfig], vk: u32, mods: Modifiers) -> Option<usize> {
    bindings.iter().position(|b| {
        b.config.trigger_vk == vk
            && b.config.ctrl == mods.ctrl
            && b.config.alt == mods.alt
            && b.config.shift == mods.shift
    })
}

// ─── Platform dispatch ────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod ll_hook;
#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub use ll_hook::{start_ll_hook, HotkeyHandle};

#[cfg(not(target_os = "windows"))]
mod linux_hotkey;
#[cfg(not(target_os = "windows"))]
#[allow(unused_imports)]
pub use linux_hotkey::{start_ll_hook, HotkeyHandle};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
