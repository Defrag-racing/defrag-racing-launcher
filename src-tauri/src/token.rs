//! Launcher token stored in the OS keyring.
//!
//! Windows → Credential Manager, macOS → Keychain, Linux → libsecret. Never
//! persisted to the JSON config. The service name is fixed per platform so
//! multiple versions of the launcher pick up the same entry after upgrade.
//!
//! Every error surfaces via `log::error!` because keyring bugs are the
//! leading support class — "I pasted my token but it disappeared" almost
//! always means Windows Defender / corporate AV blocked Credential Manager
//! access. The error message routes the user to an obvious cause.

use anyhow::{Context, Result};

const SERVICE: &str = "racing.defrag.launcher";
const USER: &str = "api-token";

pub fn save(token: &str) -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, USER)
        .context("open keyring entry (is your OS credential store available?)")
        .map_err(|e| { log::error!("keyring open failed during save: {e:?}"); e })?;
    entry
        .set_password(token)
        .context("write token to OS keyring")
        .map_err(|e| { log::error!("keyring write failed: {e:?}"); e })?;

    // Immediate readback — proves the save actually landed. Without this
    // a keyring that silently refuses writes (some sandboxed setups) would
    // look fine until the next launch.
    match entry.get_password() {
        Ok(ref t) if t == token => Ok(()),
        Ok(_) => {
            log::error!("keyring readback mismatch — stored value differs from what we wrote");
            anyhow::bail!("OS keyring saved the token but reads back a different value — refusing to continue");
        }
        Err(e) => {
            log::error!("keyring readback failed immediately after write: {e:?}");
            Err(anyhow::Error::new(e)).context("verify token after save")
        }
    }
}

pub fn load() -> Result<Option<String>> {
    let entry = keyring::Entry::new(SERVICE, USER)
        .context("open keyring entry")
        .map_err(|e| { log::error!("keyring open failed during load: {e:?}"); e })?;
    match entry.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => {
            log::error!("keyring read failed: {e:?}");
            Err(e).context("read token from keyring")
        }
    }
}

pub fn clear() -> Result<()> {
    let entry = keyring::Entry::new(SERVICE, USER).context("open keyring entry")?;
    // NoEntry is fine — "clear" is idempotent from the user's perspective.
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => {
            log::error!("keyring delete failed: {e:?}");
            Err(e).context("delete token from keyring")
        }
    }
}
