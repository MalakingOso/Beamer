#![cfg(not(target_os = "windows"))]

use super::{InjectionBackend, InjectionResult};
use anyhow::Result;
use enigo::{Enigo, Keyboard, Settings};

pub struct EnigoBackend;

impl InjectionBackend for EnigoBackend {
    fn name(&self) -> &'static str {
        "enigo"
    }

    fn display_name(&self) -> &'static str {
        "enigo (char-by-char typing)"
    }

    fn available(&self) -> Result<(), String> {
        // enigo is a compiled-in dependency, always available as last resort
        Ok(())
    }

    fn inject(&self, text: &str) -> Result<InjectionResult> {
        let mut enigo = Enigo::new(&Settings::default())?;
        enigo.text(text)?;
        Ok(InjectionResult {
            method: "enigo".into(),
            target_info: "via virtual keyboard (char-by-char)".into(),
        })
    }
}
