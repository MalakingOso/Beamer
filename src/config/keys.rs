//! API keys in the OS keyring (service "beamer"), never on disk, behind an
//! in-memory cache of keys the keyring has confirmed.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

/// Keys confirmed by the keyring, by credential name. Misses/errors are never
/// cached, so a transiently locked keyring at startup can't permanently mask a
/// key that's actually present.
static KEY_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Poison-tolerant: a panic elsewhere while holding the lock must not turn
/// every later key lookup into a panic too.
fn cache() -> MutexGuard<'static, HashMap<String, String>> {
    KEY_CACHE.lock().unwrap_or_else(|p| p.into_inner())
}

/// Read an API key, cached after the first successful read. Misses/errors
/// retry the keyring on the next call.
pub fn load_api_key(name: &str) -> String {
    if let Some(cached) = cache().get(name) {
        return cached.clone();
    }

    match keyring::Entry::new("beamer", name).and_then(|e| e.get_password()) {
        Ok(password) => {
            cache().insert(name.to_string(), password.clone());
            password
        }
        Err(_) => String::new(),
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
                cache().remove(name);
            }
            Err(_) => {}
        }
    } else if keyring::Entry::new("beamer", name)
        .and_then(|e| e.set_password(value))
        .is_ok()
    {
        cache().insert(name.to_string(), value.to_string());
    }
}
