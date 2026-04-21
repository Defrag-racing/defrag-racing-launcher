//! Persistent launcher config — engine + demos path + auto-upload flag.
//!
//! Written to JSON in the OS's standard app-config directory so it survives
//! upgrades and doesn't pollute the user's home folder. The auth token is
//! **not** stored here; it lives in the OS keyring (see `keyring.rs`).
//!
//! Backend URL defaults to production. A `DEFRAG_API_URL` env var at launch
//! time overrides it — used during local development to point at the Docker
//! Laravel instance.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_API_URL: &str = "https://defrag.racing";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Absolute path to the chosen engine binary (oDFe.x86_64, iDFe.exe, ...).
    /// None until the user picks one from the detected list or browses manually.
    pub engine_path: Option<PathBuf>,

    /// Absolute path to the folder the watcher monitors for new demos.
    /// Usually `<engine_dir>/defrag/demos/`, but user can override.
    pub demos_path: Option<PathBuf>,

    /// Master switch for background demo backup. Off by default so the
    /// launcher is harmless until the user opts in + provides a token.
    pub auto_upload_enabled: bool,

    /// First-run flag so we show onboarding exactly once and skip it
    /// afterwards, even if every individual field is still empty.
    pub onboarding_completed: bool,
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("racing", "defrag", "launcher")
            .context("could not resolve platform config directory")?;
        let dir = dirs.config_dir().to_path_buf();
        fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
        Ok(dir.join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path).with_context(|| format!("read {:?}", path))?;
        let cfg: Config = serde_json::from_str(&raw).with_context(|| format!("parse {:?}", path))?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        let raw = serde_json::to_string_pretty(self)?;
        // Write atomically via tempfile-rename so a crash mid-write doesn't
        // truncate the config to zero bytes.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, raw).with_context(|| format!("write {:?}", tmp))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
        Ok(())
    }
}

/// Backend base URL. Reads `DEFRAG_API_URL` env var once so a dev running
/// `DEFRAG_API_URL=http://localhost npm run tauri dev` points at local
/// Laravel instead of production.
pub fn api_base_url() -> String {
    std::env::var("DEFRAG_API_URL").unwrap_or_else(|_| DEFAULT_API_URL.to_string())
}

/// Guess the demos folder that lives next to a given engine binary. The
/// defrag convention is `<engine_dir>/defrag/demos/`. Returns None if the
/// path doesn't resolve to something that exists — we don't want to pre-fill
/// the field with a made-up path.
pub fn guess_demos_path_from_engine(engine: &Path) -> Option<PathBuf> {
    let candidate = engine.parent()?.join("defrag").join("demos");
    if candidate.exists() { Some(candidate) } else { None }
}
