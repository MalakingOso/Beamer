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

/// Cap on retained transcripts.
///
/// `save()` rewrites the whole file after every injection, so an uncapped log
/// meant that cost grew without bound over the app's lifetime — each dictation
/// re-serializing every dictation that came before it. 1000 entries keeps the
/// file at a few hundred KB and the rewrite in the low milliseconds, while
/// still covering months of ordinary use. `StatusLog` caps itself the same way.
const MAX_ENTRIES: usize = 1000;

impl TranscriptionHistory {
    fn storage_path() -> PathBuf {
        Config::config_dir().join("history.json")
    }

    pub fn load() -> Self {
        let path = Self::storage_path();
        if !path.exists() {
            return Self::default();
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read history at {:?}: {}", path, e);
                return Self::default();
            }
        };
        match serde_json::from_str::<TranscriptionHistory>(&contents) {
            Ok(mut history) => {
                history.path = path;
                history
            }
            Err(e) => {
                // Don't silently start from empty: the next `append` would
                // overwrite the unreadable file and destroy whatever was in
                // it for good. Move it aside so it's recoverable by hand.
                let backup = path.with_extension("json.corrupt");
                tracing::error!(
                    "History at {:?} is not valid JSON ({}); preserving it as {:?} and starting fresh",
                    path, e, backup
                );
                if let Err(e) = std::fs::rename(&path, &backup) {
                    tracing::error!("Could not preserve corrupt history: {}", e);
                }
                Self::default()
            }
        }
    }

    /// Persist the log, replacing the file atomically.
    ///
    /// Writes to a sibling temp file and renames over the target, so a crash
    /// or a full disk mid-write leaves the previous history intact rather than
    /// a half-written file that `load()` would then reject.
    pub fn save(&self) -> Result<()> {
        let dir = Config::config_dir();
        std::fs::create_dir_all(&dir)?;
        let contents = serde_json::to_string_pretty(self)?;

        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, contents)?;
        if let Err(e) = crate::notes::sync_doc::rename_with_retry(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    pub fn append(&mut self, text: String) {
        let timestamp = Local::now().to_rfc3339();
        self.entries.push(HistoryEntry { timestamp, text });
        if self.entries.len() > MAX_ENTRIES {
            let excess = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..excess);
        }
        if let Err(e) = self.save() {
            tracing::error!("Failed to save transcription history: {}", e);
        }
    }

    pub fn recent(&self, n: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(n).collect()
    }

    /// Groups entries by calendar day, cloning entries so the result is
    /// independent of `self` (suitable for caching in a `use_memo`).
    /// Each entry's timestamp is parsed exactly once per call.
    pub fn grouped_by_day(&self) -> Vec<(String, Vec<HistoryEntry>)> {
        let today = Local::now().date_naive();
        let yesterday = today.pred_opt().unwrap_or(today);

        let mut groups: BTreeMap<NaiveDate, Vec<HistoryEntry>> = BTreeMap::new();
        for entry in &self.entries {
            let date = DateTime::parse_from_rfc3339(&entry.timestamp)
                .map(|dt| dt.with_timezone(&Local).date_naive())
                .unwrap_or(today);
            groups.entry(date).or_default().push(entry.clone());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a history rooted at a PID-scoped temp path so concurrent test
    /// runs don't race, and nothing touches the real user config dir.
    fn temp_history(tag: &str) -> TranscriptionHistory {
        let dir = std::env::temp_dir().join(format!("beamer_history_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TranscriptionHistory {
            entries: Vec::new(),
            path: dir.join(format!("{tag}.json")),
        }
    }

    #[test]
    fn append_evicts_oldest_beyond_the_cap() {
        let mut history = temp_history("cap");
        for i in 0..(MAX_ENTRIES + 10) {
            history.append(format!("entry {i}"));
        }

        assert_eq!(history.entries.len(), MAX_ENTRIES, "log must stay bounded");
        // The 10 oldest are gone; the newest is still last.
        assert_eq!(history.entries.first().unwrap().text, "entry 10");
        assert_eq!(
            history.entries.last().unwrap().text,
            format!("entry {}", MAX_ENTRIES + 9)
        );

        let _ = std::fs::remove_file(&history.path);
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let mut history = temp_history("atomic");
        history.append("hello".to_string());

        assert!(history.path.exists(), "history file should exist after append");
        assert!(
            !history.path.with_extension("json.tmp").exists(),
            "the temp file must be renamed away, not left on disk"
        );

        let _ = std::fs::remove_file(&history.path);
    }

    #[test]
    fn saved_history_round_trips() {
        let mut history = temp_history("roundtrip");
        history.append("first".to_string());
        history.append("second".to_string());

        let contents = std::fs::read_to_string(&history.path).unwrap();
        let reloaded: TranscriptionHistory = serde_json::from_str(&contents).unwrap();
        let texts: Vec<_> = reloaded.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "second"]);

        let _ = std::fs::remove_file(&history.path);
    }
}
