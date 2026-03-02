use anyhow::Result;
use chrono::{DateTime, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::Config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub text: String,
}

/// Append-only transcription log persisted as JSON in `%APPDATA%/Beamer/history.json`.
/// Loaded on startup, appended after each successful injection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptionHistory {
    pub entries: Vec<HistoryEntry>,
    #[serde(skip)]
    path: PathBuf,
}

impl Default for TranscriptionHistory {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            path: Self::storage_path(),
        }
    }
}

impl TranscriptionHistory {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("history.json")
    }

    pub fn load() -> Self {
        let path = Self::storage_path();
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(contents) => {
                    let mut history: TranscriptionHistory =
                        serde_json::from_str(&contents).unwrap_or_default();
                    history.path = path;
                    history
                }
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = Config::config_dir();
        std::fs::create_dir_all(&dir)?;
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&self.path, contents)?;
        Ok(())
    }

    pub fn append(&mut self, text: String) {
        let timestamp = Local::now().to_rfc3339();
        self.entries.push(HistoryEntry { timestamp, text });
        let _ = self.save();
    }

    pub fn recent(&self, n: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(n).collect()
    }

    pub fn grouped_by_day(&self) -> Vec<(String, Vec<&HistoryEntry>)> {
        let today = Local::now().date_naive();
        let yesterday = today.pred_opt().unwrap_or(today);

        let mut groups: BTreeMap<NaiveDate, Vec<&HistoryEntry>> = BTreeMap::new();
        for entry in &self.entries {
            let date = DateTime::parse_from_rfc3339(&entry.timestamp)
                .map(|dt| dt.with_timezone(&Local).date_naive())
                .unwrap_or(today);
            groups.entry(date).or_default().push(entry);
        }

        groups
            .into_iter()
            .rev()
            .map(|(date, entries)| {
                let label = if date == today {
                    "Today".to_string()
                } else if date == yesterday {
                    "Yesterday".to_string()
                } else {
                    date.format("%B %d, %Y").to_string()
                };
                (label, entries)
            })
            .collect()
    }
}
