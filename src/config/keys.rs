//! API keys in the OS keyring (service "beamer"), never on disk, behind an
//! in-memory cache of keys the keyring has confirmed.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

struct KeyCache {
    entries: HashMap<String, String>,
    /// Monotonic counter bumped on every write/delete so a concurrent slow
    /// `load_api_key` cannot overwrite a newer `save_api_key` result.
    generation: u64,
}

/// Keys confirmed by the keyring, by credential name. Misses/errors are never
/// cached, so a transiently locked keyring at startup can't permanently mask a
/// key that's actually present.
static KEY_CACHE: LazyLock<Mutex<KeyCache>> = LazyLock::new(|| {
    Mutex::new(KeyCache {
        entries: HashMap::new(),
        generation: 0,
    })
});

/// Poison-tolerant: a panic elsewhere while holding the lock must not turn
/// every later key lookup into a panic too.
fn cache() -> MutexGuard<'static, KeyCache> {
    KEY_CACHE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Read an API key, cached after the first successful read. Misses/errors
/// retry the keyring on the next call.
pub fn load_api_key(name: &str) -> String {
    let gen_before = {
        let guard = cache();
        if let Some(cached) = guard.entries.get(name) {
            return cached.clone();
        }
        guard.generation
    };

    match keyring::Entry::new("beamer", name).and_then(|e| e.get_password()) {
        Ok(password) => {
            let mut guard = cache();
            if guard.generation == gen_before {
                guard.entries.insert(name.to_string(), password.clone());
            }
            password
        }
        Err(keyring::Error::NoEntry) => String::new(),
        Err(e) => {
            tracing::warn!("Could not read '{}' from OS keyring: {}", name, e);
            String::new()
        }
    }
}

/// Write (or delete, when `value` is empty) an API key in the OS keyring.
/// The cache updates only after the keyring confirms, so it never claims
/// state that isn't durably stored.
pub fn save_api_key(name: &str, value: &str) {
    if value.is_empty() {
        let result = keyring::Entry::new("beamer", name).and_then(|e| e.delete_credential());
        match result {
            // Deleted or already absent — either way, drop the cached value.
            Ok(()) | Err(keyring::Error::NoEntry) => {
                let mut guard = cache();
                guard.generation = guard.generation.wrapping_add(1);
                guard.entries.remove(name);
            }
            Err(e) => {
                tracing::error!("Failed to delete '{}' from OS keyring: {}", name, e);
            }
        }
    } else {
        match keyring::Entry::new("beamer", name).and_then(|e| e.set_password(value)) {
            Ok(()) => {
                let mut guard = cache();
                guard.generation = guard.generation.wrapping_add(1);
                guard.entries.insert(name.to_string(), value.to_string());
            }
            Err(e) => {
                tracing::error!("Failed to save '{}' to OS keyring: {}", name, e);
            }
        }
    }
}
