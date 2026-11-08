//! The user's cloud API keys. On macOS they live in the login keychain
//! (one generic password per provider), so they never sit in a settings
//! file; the panel can save or remove a key but never shows one again.
//! Elsewhere (no keychain wired up yet) a key lasts for this run only.

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "Reel Studio AI agent";

/// Keys kept for this run (other systems, or a keychain that refused).
static MEMORY: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn memory<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// Saved keys survive a restart.
pub fn persistent() -> bool {
    cfg!(target_os = "macos")
}

/// Saves (or, with "", removes) the key for `provider`.
pub fn save(provider: &str, key: &str) -> anyhow::Result<()> {
    let key = key.trim();
    #[cfg(target_os = "macos")]
    {
        use security_framework::passwords::{delete_generic_password, set_generic_password};
        if key.is_empty() {
            let _ = delete_generic_password(SERVICE, provider);
        } else {
            set_generic_password(SERVICE, provider, key.as_bytes())?;
        }
    }
    memory(|m| {
        if key.is_empty() {
            m.remove(provider);
        } else {
            m.insert(provider.into(), key.into());
        }
    });
    Ok(())
}

/// The stored key (only the request builder asks).
pub fn get(provider: &str) -> Option<String> {
    if let Some(k) = memory(|m| m.get(provider).cloned()) {
        return Some(k);
    }
    #[cfg(target_os = "macos")]
    {
        use security_framework::passwords::get_generic_password;
        if let Ok(bytes) = get_generic_password(SERVICE, provider) {
            let key = String::from_utf8(bytes).ok()?;
            // Asked once per run: the keychain may prompt.
            memory(|m| m.insert(provider.into(), key.clone()));
            return Some(key);
        }
    }
    None
}

pub fn has(provider: &str) -> bool {
    get(provider).is_some_and(|k| !k.is_empty())
}
