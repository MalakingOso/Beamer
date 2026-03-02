use dioxus::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusEntry {
    pub time: String,
    pub level: LogLevel,
    pub message: String,
}

const MAX_ENTRIES: usize = 50;

/// Ring-buffer-style log shown in the Debug card. Oldest entries are evicted
/// when the buffer exceeds `MAX_ENTRIES`.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusLog {
    pub entries: Vec<StatusEntry>,
}

impl StatusLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, level: LogLevel, message: impl Into<String>) {
        let now = chrono::Local::now().format("%H:%M:%S").to_string();
        self.entries.push(StatusEntry {
            time: now,
            level,
            message: message.into(),
        });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
    }
}

/// Helper to push a status entry into a signal from async code.
pub fn log_status(log: &mut Signal<StatusLog>, level: LogLevel, message: impl Into<String>) {
    log.write().push(level, message);
}
