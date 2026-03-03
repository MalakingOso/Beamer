use anyhow::Result;
use std::path::PathBuf;

/// Custom vocabulary terms sent to STT backends to improve recognition of
/// domain-specific words (product names, jargon, etc.). Stored as one term
/// per line in `%APPDATA%/Beamer/vocabulary.txt`.
pub struct Vocabulary {
    terms: Vec<String>,
    path: PathBuf,
}

impl Vocabulary {
    fn path() -> PathBuf {
        crate::config::Config::config_dir().join("vocabulary.txt")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        let terms = if path.exists() {
            std::fs::read_to_string(&path)?
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect()
        } else {
            Vec::new()
        };
        Ok(Self { terms, path })
    }

    pub fn list(&self) -> &[String] {
        &self.terms
    }

    pub fn add(&mut self, term: &str) -> Result<()> {
        let term = term.trim().to_string();
        if !term.is_empty() && !self.terms.contains(&term) {
            self.terms.push(term);
            self.save()?;
        }
        Ok(())
    }

    pub fn remove(&mut self, term: &str) -> Result<()> {
        self.terms.retain(|t| t != term);
        self.save()?;
        Ok(())
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let contents = self.terms.join("\n");
        std::fs::write(&self.path, contents)?;
        Ok(())
    }
}
