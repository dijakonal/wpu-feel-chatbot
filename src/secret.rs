//! Secrets live in the OS credential store (Windows Credential Manager / Linux Secret Service,
//! e.g. GNOME Keyring or KWallet), not in settings.json.
//!
//! Systems without a credential store keep working: the caller then falls back to the file.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};

const SERVICE: &str = "ais-chat-client";

/// What was last written to / read from the store, per name – so the store is only touched
/// when a value really changes (on Linux every access can ask to unlock the keyring).
fn known() -> &'static Mutex<HashMap<String, String>> {
    static KNOWN: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

/// Values that could not be stored, per name – not retried until the value changes.
fn failed() -> &'static Mutex<HashMap<String, String>> {
    static FAILED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    FAILED.get_or_init(Default::default)
}

/// Set once the secrets saved earlier were read at start and handed to the settings. Until
/// then an empty value in the settings only means "not read yet", not "removed".
static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mark_loaded() {
    LOADED.store(true, std::sync::atomic::Ordering::SeqCst);
}

pub fn loaded() -> bool {
    LOADED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Whether saving the settings should write `value` to the store: an empty value deletes the
/// stored one, which is only right once the stored secrets were read (otherwise the empty
/// value just means they have not arrived yet – deleting them then lost the user's tokens).
pub fn should_store(value: &str, loaded: bool) -> bool {
    !value.is_empty() || loaded
}

/// Save `value` under `name` (an empty value removes it). Can block like [`load`].
pub fn store(name: &str, value: &str) -> Result<()> {
    let same = |m: &Mutex<HashMap<String, String>>| m.lock().map(|k| k.get(name).map(String::as_str) == Some(value)).unwrap_or(false);
    if same(known()) {
        return Ok(());
    }
    if same(failed()) {
        anyhow::bail!("No system password store available");
    }
    let result = (|| -> Result<()> {
        let entry = keyring::Entry::new(SERVICE, name).context("Could not open the system password store")?;
        if value.is_empty() {
            let _ = entry.delete_credential(); // nothing to delete is fine
            Ok(())
        } else {
            entry.set_password(value).context("Could not save the secret in the system password store")
        }
    })();
    let map = if result.is_ok() { known() } else { failed() };
    if let Ok(mut k) = map.lock() {
        k.insert(name.to_string(), value.to_string());
    }
    result
}

/// Where a value is, as far as known without touching the store (never blocks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    /// In the password store.
    Stored,
    /// The store could not take it (none on this system).
    Unavailable,
    /// Not tried yet / still being saved.
    Unknown,
}

pub fn status(name: &str, value: &str) -> Status {
    let has = |m: &Mutex<HashMap<String, String>>| m.lock().map(|k| k.get(name).map(String::as_str) == Some(value)).unwrap_or(false);
    if has(known()) {
        Status::Stored
    } else if has(failed()) {
        Status::Unavailable
    } else {
        Status::Unknown
    }
}

/// The value saved under `name`, if the store has one. Can block for a long time (D-Bus may
/// wait for a password store to start) – call it on a background thread.
pub fn load(name: &str) -> Option<String> {
    let value = keyring::Entry::new(SERVICE, name).ok()?.get_password().ok()?;
    if let Ok(mut k) = known().lock() {
        k.insert(name.to_string(), value.clone());
    }
    Some(value)
}
