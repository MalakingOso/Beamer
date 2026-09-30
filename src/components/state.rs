//! `components.json` in the config dir: what was last applied, per component
//! id. `plan` treats a component as current only if its record matches the
//! catalog sha256. Written only after a whole group applied and its server
//! started, so a record always describes a consistent set. Same shape as
//! `notes::machine::MachineStore`: serde defaults, `.corrupt` quarantine,
//! `.tmp` + rename save, and a stat failure latches read-only rather than
//! looking like a first run.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub sha256: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ComponentStore {
    #[serde(default)]
    components: BTreeMap<String, Record>,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    read_only: bool,
}

impl ComponentStore {
    pub fn default_path() -> PathBuf {
        crate::config::Config::config_dir().join("components.json")
    }

    /// Missing or corrupt: empty (corrupt is quarantined). Unstat-able:
    /// empty and read-only, so nothing overwrites whatever is really there.
    pub fn load_from(path: PathBuf) -> Self {
        let empty = |path: PathBuf, read_only: bool| Self { path, read_only, ..Self::default() };
        match path.try_exists() {
            Ok(false) => return empty(path, false),
            Err(e) => {
                tracing::error!(
                    "Could not tell whether {:?} exists ({}); treating every component as stale \
                     and leaving the file alone",
                    path, e
                );
                return empty(path, true);
            }
            Ok(true) => {}
        }
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Could not read {:?}: {}", path, e);
                return empty(path, false);
            }
        };
        match serde_json::from_str::<ComponentStore>(&contents) {
            Ok(mut store) => {
                store.path = path;
                store
            }
            Err(e) => {
                let backup = path.with_extension("json.corrupt");
                tracing::error!("{:?} is not valid JSON ({}); preserving as {:?}", path, e, backup);
                let _ = std::fs::rename(&path, &backup);
                empty(path, false)
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        self.components.get(id)
    }

    pub fn set(&mut self, id: &str, record: Record) {
        self.components.insert(id.to_string(), record);
    }

    pub fn save(&self) -> Result<()> {
        if self.read_only {
            anyhow::bail!("{:?} could not be stat-ed at load; not overwriting it", self.path);
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        if let Err(e) = crate::notes::sync_doc::rename_with_retry(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("beamer-components-state-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("components.json")
    }

    fn record(sha: &str) -> Record {
        Record { version: "v".into(), sha256: sha.into() }
    }

    #[test]
    fn records_round_trip_through_disk() {
        let path = temp_path("round-trip");
        let mut store = ComponentStore::load_from(path.clone());
        store.set("a", record("aa"));
        store.save().unwrap();
        let reloaded = ComponentStore::load_from(path.clone());
        assert_eq!(reloaded.get("a"), Some(&record("aa")));
        assert!(!path.with_extension("json.tmp").exists(), "the temp file is renamed away");
    }

    #[test]
    fn a_corrupt_file_is_quarantined_not_discarded() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{ not json").unwrap();
        let store = ComponentStore::load_from(path.clone());
        assert!(store.get("a").is_none());
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.corrupt")).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn unknown_fields_and_missing_ones_still_load() {
        let path = temp_path("lenient");
        std::fs::write(&path, r#"{"components":{"a":{"sha256":"aa","extra":1}},"future":true}"#).unwrap();
        let store = ComponentStore::load_from(path);
        assert_eq!(store.get("a").map(|r| r.sha256.as_str()), Some("aa"));
    }
}
