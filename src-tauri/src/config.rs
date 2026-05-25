//! Persistent launcher config - engine + demos path + auto-upload flag.
//!
//! Written to JSON in the OS's standard app-config directory so it survives
//! upgrades and doesn't pollute the user's home folder. The auth token is
//! **not** stored here; it lives in the OS keyring (see `keyring.rs`).
//!
//! Backend URL defaults to production. A `DEFRAG_API_URL` env var at launch
//! time overrides it - used during local development to point at the Docker
//! Laravel instance.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_API_URL: &str = "https://defrag.racing";

/// Default CPU target for the hashing throttle. 15% keeps the launcher
/// invisible during gameplay even on weaker hardware - the user can
/// crank it up via the Speed-up button or Settings when they want a
/// big rescan done quickly.
pub const DEFAULT_CPU_THROTTLE_PCT: u8 = 15;

fn default_true() -> bool { true }
fn default_cpu_throttle_pct() -> u8 { DEFAULT_CPU_THROTTLE_PCT }

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Recurse into subdirectories of demos_path when watching/scanning.
    /// Off by default because Defrag itself writes demos to the top level
    /// and recursive watching would surprise users who archive old demos
    /// into subfolders specifically to STOP backing them up. Turning it
    /// on covers organized demos folders ("by date", "by map", etc.)
    /// without forcing the user to flatten everything.
    #[serde(default)]
    pub include_subfolders: bool,

    /// Auto-update opt-in. On by default - security fixes need to reach
    /// users without them having to remember to check Releases. Users
    /// can flip it off in Settings if they want manual control over
    /// when binaries change.
    #[serde(default = "default_true")]
    pub auto_update_enabled: bool,

    /// Target CPU percentage the hash worker is allowed to use, as a
    /// duty-cycle. 0 = no throttle (full speed). Value flows into
    /// UploadState at watcher::start; the Speed-up button on Dashboard
    /// temporarily overrides it without touching this saved value.
    /// 15% by default - see DEFAULT_CPU_THROTTLE_PCT.
    #[serde(default = "default_cpu_throttle_pct")]
    pub cpu_throttle_pct: u8,

    /// First-run flag so we show onboarding exactly once and skip it
    /// afterwards, even if every individual field is still empty.
    pub onboarding_completed: bool,

    /// Version of the launcher that last wrote this config. Lets us detect
    /// "you just upgraded / reinstalled" on startup so the user can choose
    /// to start fresh or keep their settings without manually running
    /// Reset. None = config written before we added this field (pre-0.1.3).
    #[serde(default)]
    pub config_version: Option<String>,
}

impl Default for Config {
    // Manual impl rather than #[derive(Default)] because auto_update_enabled
    // needs to default to true. Everything else uses Default::default() for
    // its type - Option/bool/etc - which the field declarations document.
    fn default() -> Self {
        Self {
            engine_path: None,
            demos_path: None,
            auto_upload_enabled: false,
            include_subfolders: false,
            auto_update_enabled: true,
            cpu_throttle_pct: DEFAULT_CPU_THROTTLE_PCT,
            onboarding_completed: false,
            config_version: None,
        }
    }
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
        // Stamp the current launcher version so the next boot can tell
        // whether the config was last touched by this version or an older
        // one - drives the "Previous install detected" dialog.
        let mut to_write = self.clone();
        to_write.config_version = Some(env!("CARGO_PKG_VERSION").to_string());
        let raw = serde_json::to_string_pretty(&to_write)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, raw).with_context(|| format!("write {:?}", tmp))?;
        fs::rename(&tmp, &path).with_context(|| format!("rename to {:?}", path))?;
        Ok(())
    }
}

/// Returns Some(previous_version) if the persisted config was written by
/// a launcher version different from the one running now, None otherwise.
///
/// Interprets a missing `config_version` field as "pre-0.1.3 config"
/// (which is when the field was added) so the upgrade prompt fires once
/// after users upgrade into a version that knows about the field.
pub fn previous_version(cfg: &Config) -> Option<String> {
    let current = env!("CARGO_PKG_VERSION");
    match &cfg.config_version {
        Some(v) if v != current => Some(v.clone()),
        None if cfg.onboarding_completed => Some("pre-0.1.3".to_string()),
        _ => None,
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
/// path doesn't resolve to something that exists - we don't want to pre-fill
/// the field with a made-up path.
pub fn guess_demos_path_from_engine(engine: &Path) -> Option<PathBuf> {
    let candidate = engine.parent()?.join("defrag").join("demos");
    if candidate.exists() { Some(candidate) } else { None }
}
