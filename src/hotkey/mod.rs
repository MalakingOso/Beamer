/// Sent from the hotkey listener to the orchestrator coroutine.
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    RecordStart,
    RecordStop,
}
