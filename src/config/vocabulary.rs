use anyhow::Result;
use std::path::PathBuf;

pub struct Vocabulary {
    terms: Vec<String>,
    path: PathBuf,
}

impl Vocabulary {
    pub fn path() -> PathBuf {
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

    pub fn import_from_file(&mut self, file_path: &std::path::Path) -> Result<usize> {
        let contents = std::fs::read_to_string(file_path)?;
        let mut count = 0;
        for line in contents.lines() {
            let term = line.trim().to_string();
            if !term.is_empty() && !self.terms.contains(&term) {
                self.terms.push(term);
                count += 1;
            }
        }
        if count > 0 {
            self.save()?;
        }
        Ok(count)
    }

    fn save(&self) -> Result<()> {
        let dir = self.path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let contents = self.terms.join("\n");
        std::fs::write(&self.path, contents)?;
        Ok(())
    }
}
