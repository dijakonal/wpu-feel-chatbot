//! Secrets live in the OS credential store (Windows Credential Manager / Linux Secret Service),
//! not in settings.json.

use anyhow::{Context, Result};

const SERVICE: &str = "ais-chat-client";

pub fn store(name: &str, value: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, name).context("Could not open the system keystore")?;
    if value.is_empty() {
        let _ = entry.delete_credential(); // nothing to delete is fine
        Ok(())
    } else {
        entry.set_password(value).context("Could not save the secret in the system keystore")
    }
}

pub fn load(name: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, name).ok()?.get_password().ok()
}
