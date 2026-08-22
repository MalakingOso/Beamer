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

    /// Replace `old` with `new` **in place**.
    ///
    /// The UI used to rename by calling `remove(old)` then `add(new)`, which
    /// appends — so on disk the term jumped to the end of the list while the
    /// in-memory list kept it where it was, and the two disagreed until the
    /// next restart. A no-op when `old` isn't present or `new` is blank; if
    /// `new` already exists elsewhere in the list, `old` is simply dropped so
    /// the rename can't introduce a duplicate.
    pub fn rename(&mut self, old: &str, new: &str) -> Result<()> {
        let new = new.trim();
        if new.is_empty() || old == new {
            return Ok(());
        }
        let Some(idx) = self.terms.iter().position(|t| t == old) else {
            return Ok(());
        };
        if self.terms.iter().any(|t| t == new) {
            self.terms.remove(idx);
        } else {
            self.terms[idx] = new.to_string();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A vocabulary rooted in a PID-scoped temp file, so tests never touch the
    /// real `%APPDATA%`/`~/.config` vocabulary and concurrent runs don't race.
    fn temp_vocab(tag: &str, terms: &[&str]) -> Vocabulary {
        let dir = std::env::temp_dir().join(format!("beamer_vocab_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Vocabulary {
            terms: terms.iter().map(|s| s.to_string()).collect(),
            path: dir.join(format!("{tag}.txt")),
        }
    }

    #[test]
    fn rename_keeps_the_term_in_place() {
        let mut v = temp_vocab("inplace", &["alpha", "beemer", "gamma"]);
        v.rename("beemer", "Beamer").unwrap();
        assert_eq!(v.list(), ["alpha", "Beamer", "gamma"]);

        // And the file agrees — this is the half that used to drift.
        let on_disk = std::fs::read_to_string(&v.path).unwrap();
        assert_eq!(on_disk, "alpha\nBeamer\ngamma");
        let _ = std::fs::remove_file(&v.path);
    }

    #[test]
    fn rename_to_an_existing_term_drops_the_duplicate() {
        let mut v = temp_vocab("dupe", &["alpha", "beemer", "Beamer"]);
        v.rename("beemer", "Beamer").unwrap();
        assert_eq!(v.list(), ["alpha", "Beamer"]);
        let _ = std::fs::remove_file(&v.path);
    }

    #[test]
    fn rename_of_a_missing_term_is_a_no_op() {
        let mut v = temp_vocab("missing", &["alpha"]);
        v.rename("nope", "something").unwrap();
        assert_eq!(v.list(), ["alpha"]);
    }

    #[test]
    fn rename_to_blank_or_identical_is_a_no_op() {
        let mut v = temp_vocab("blank", &["alpha", "beta"]);
        v.rename("alpha", "   ").unwrap();
        v.rename("beta", "beta").unwrap();
        assert_eq!(v.list(), ["alpha", "beta"]);
    }

    #[test]
    fn rename_trims_surrounding_whitespace() {
        let mut v = temp_vocab("trim", &["alpha"]);
        v.rename("alpha", "  Alpha  ").unwrap();
        assert_eq!(v.list(), ["Alpha"]);
        let _ = std::fs::remove_file(&v.path);
    }
}
