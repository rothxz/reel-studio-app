//! The user's cloud API keys, kept in the system's credential store so they
//! never sit in a settings file; the panel can save or remove a key but
//! never shows one again.
//!
//! - macOS: the login keychain (one generic password per provider), read
//!   on first use.
//! - Linux, Windows: the Secret Service (GNOME Keyring, KWallet…) or the
//!   Windows Credential Manager, through gpui's credentials API. That is
//!   asynchronous, so `load` reads every provider's key at launch and
//!   `save` writes through in the background; the calls below read the
//!   copy kept for the run.
//!
//! Where the store refuses (no keyring daemon), a key lasts for this run.

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
const SERVICE: &str = "Reel Studio AI agent";

/// Keys kept for this run (read from the store, or saved this run).
static MEMORY: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn memory<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// The credential-store entry of a provider (gpui's credentials API names
/// them by URL).
#[cfg(not(target_os = "macos"))]
fn entry(provider: &str) -> String {
    format!("reel-studio-agent://{provider}")
}

/// Saved keys survive a restart (a store exists on every platform; a
/// Linux desktop without a keyring daemon is the exception, logged).
pub fn persistent() -> bool {
    true
}

/// Reads the saved keys at launch (Linux, Windows; macOS reads lazily).
pub fn load(cx: &mut gpui::App) {
    #[cfg(not(target_os = "macos"))]
    for preset in super::providers::PRESETS.iter().filter(|p| p.cloud) {
        let id = preset.id;
        let read = cx.read_credentials(&entry(id));
        cx.background_executor()
            .spawn(async move {
                match read.await {
                    Ok(Some((_, secret))) => {
                        if let Ok(key) = String::from_utf8(secret) {
                            // A key saved this run (before the read came back) wins.
                            memory(|m| {
                                m.entry(id.into()).or_insert(key);
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("couldn't read the {id} key: {e:#}"),
                }
            })
            .detach();
    }
    #[cfg(target_os = "macos")]
    let _ = cx;
}

/// Saves (or, with "", removes) the key for `provider`.
pub fn save(provider: &str, key: &str, cx: &mut gpui::App) -> anyhow::Result<()> {
    let key = key.trim();
    #[cfg(target_os = "macos")]
    {
        use security_framework::passwords::{delete_generic_password, set_generic_password};
        if key.is_empty() {
            let _ = delete_generic_password(SERVICE, provider);
        } else {
            set_generic_password(SERVICE, provider, key.as_bytes())?;
        }
        let _ = cx;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let url = entry(provider);
        let write = if key.is_empty() {
            cx.delete_credentials(&url)
        } else {
            cx.write_credentials(&url, provider, key.as_bytes())
        };
        let name = provider.to_string();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = write.await {
                    eprintln!("the {name} key is kept for this run only: {e:#}");
                }
            })
            .detach();
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
