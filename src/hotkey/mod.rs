/// Sent from the hotkey listener to the orchestrator coroutine.
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    RecordStart,
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

        let trigger_vk = if has_win && key_str.is_empty() {
            VK_LWIN
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

// ─── Platform dispatch ────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod ll_hook;
#[cfg(target_os = "windows")]
pub use ll_hook::{start_ll_hook, HotkeyHandle};

#[cfg(not(target_os = "windows"))]
mod linux_hotkey;
#[cfg(not(target_os = "windows"))]
#[allow(unused_imports)]
pub use linux_hotkey::{start_ll_hook, HotkeyHandle};
