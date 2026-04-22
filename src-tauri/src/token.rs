//! Launcher token storage.
//!
//! File-based, living in the same platform config directory as config.json.
//! Earlier revisions used the OS keyring (Credential Manager / Keychain /
//! libsecret) but on Windows several testers hit silent write failures
//! (corporate AV + Defender flavors blocking Credential Manager API for
//! non-MS-signed apps) — the save would return Ok but the next read would
//! see NoEntry. File storage sidesteps the whole class of problems and
//! the token lives in the user's own AppData\Local, no less secure than
//! any other per-user file.
//!
//! The file is base64 of the raw token — purely to keep the token from
//! appearing verbatim in a filesystem scan (security-theatre, not real
//! encryption). Readers/writers do not try to lock the file; two
//! simultaneous writes from launcher instances would be a user error
//! (nobody runs two launchers).

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use std::fs;
use std::path::PathBuf;

fn token_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("racing", "defrag", "launcher")
        .context("could not resolve platform config directory")?;
    let dir = dirs.config_dir().to_path_buf();
    fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
    Ok(dir.join("token"))
}

pub fn save(token: &str) -> Result<()> {
    let path = token_path()?;
    let encoded = B64.encode(token.as_bytes());

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &encoded).with_context(|| format!("write {:?}", tmp))?;
    fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;

    // Best-effort chmod on Unix. Windows users rely on AppData\Local ACLs.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    // Immediate readback — proves the save actually landed. Catches weird
    // FS issues (disk full, read-only mount, AV quarantine).
    match load() {
        Ok(Some(t)) if t == token => {
            log::info!("token saved successfully ({} bytes)", token.len());
            Ok(())
        }
        Ok(Some(_)) => {
            log::error!("token readback mismatch — disk corruption or race?");
            anyhow::bail!("token file read back a different value — refusing to continue")
        }
        Ok(None) => {
            log::error!("token file vanished immediately after save");
            anyhow::bail!("token file was saved but cannot be read back")
        }
        Err(e) => {
            log::error!("token readback failed: {e:?}");
            Err(e).context("verify token after save")
        }
    }
}

pub fn load() -> Result<Option<String>> {
    let path = token_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let encoded = fs::read_to_string(&path).with_context(|| format!("read {:?}", path))?;
    let bytes = B64
        .decode(encoded.trim())
        .with_context(|| format!("decode {:?}", path))?;
    let token = String::from_utf8(bytes).context("token is not valid utf-8")?;
    Ok(Some(token))
}

pub fn clear() -> Result<()> {
    let path = token_path()?;
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("delete {:?}", path))?;
    }
    Ok(())
}
