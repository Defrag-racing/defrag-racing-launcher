//! Launcher token stored in the OS keyring.
//!
//! Windows → Credential Manager, macOS → Keychain, Linux → libsecret. Never
//! persisted to the JSON config. The service name is fixed per platform so
//! multiple versions of the launcher pick up the same entry after upgrade.

use anyhow::{Context, Result};

const SERVICE: &str = "racing.defrag.launcher";
const USER: &str = "api-token";

pub fn save(token: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, USER).context("open keyring entry")?;
    entry.set_password(token).context("write token to keyring")?;
    Ok(())
}

pub fn load() -> Result<Option<String>> {
    let entry = keyring::Entry::new(SERVICE, USER).context("open keyring entry")?;
    match entry.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("read token from keyring"),
    }
}

pub fn clear() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, USER).context("open keyring entry")?;
    // NoEntry is fine — "clear" is idempotent from the user's perspective.
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("delete token from keyring"),
    }
}
