pub mod ll_hook;
pub use ll_hook::{start_ll_hook, HotkeyConfig};

/// Sent from the hotkey listener to the orchestrator coroutine.
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    RecordStart,
    RecordStop,
}
