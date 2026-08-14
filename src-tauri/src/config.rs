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

    /// Skip the "Connect to X.X.X.X?" confirmation banner and launch
    /// the engine immediately when a defrag:// URL arrives. Off by
    /// default because an accidental click on a defrag link in a chat
    /// or forum would otherwise yeet the user straight into a random
    /// server - users who join often and trust their sources can opt
    /// in via Settings. Has no effect when no engine is configured;
    /// the banner still appears so the user can see what failed.
    #[serde(default)]
    pub deep_link_auto_connect: bool,

    /// First-run flag so we show onboarding exactly once and skip it
    /// afterwards, even if every individual field is still empty.
    pub onboarding_completed: bool,

    /// Version of the launcher that last wrote this config. Lets us detect
    /// "you just upgraded / reinstalled" on startup so the user can choose
    /// to start fresh or keep their settings without manually running
    /// Reset. None = config written before we added this field (pre-0.1.3).
    #[serde(default)]
    pub config_version: Option<String>,

    /// Developer mode. Reveals the advanced launch surface in Settings:
    /// custom engine arguments + named quick-launch profiles. Off by
    /// default - it's power-user territory that would only clutter the
    /// normal setup. When off, `custom_launch_args` and `launch_profiles`
    /// are ignored even if present, so toggling it back off cleanly hides
    /// the feature without discarding what the user typed.
    #[serde(default)]
    pub developer_mode: bool,

    /// Extra arguments appended to the standard Quick launch, as a single
    /// free-form, shell-style string (quotes respected so a value with a
    /// space stays one argument). Empty = none. Only honoured in developer
    /// mode.
    #[serde(default)]
    pub custom_launch_args: String,

    /// User-defined named launch profiles, each carrying its own argument
    /// string. Surfaced as extra quick-launch entries next to the main
    /// one. Only honoured in developer mode.
    #[serde(default)]
    pub launch_profiles: Vec<LaunchProfile>,

    /// What happens to a demo that looks like a run of this week's comps map.
    /// `ask` by default: the demo is held and the user picks. See
    /// comps::CompsMode for why holding is the safe default rather than a
    /// cautious one.
    #[serde(default)]
    pub comps_mode: crate::comps::CompsMode,

    /// Whether the user has already been shown what holding a demo means.
    /// The explanation appears once, the first time the guard actually holds
    /// something - explaining it during onboarding would describe a situation
    /// the user has not been in yet.
    #[serde(default)]
    pub comps_intro_seen: bool,

    /// Whether the user has been asked, once, whether the launcher should open
    /// `.dm_68` files.
    ///
    /// Asked in the app rather than in the installer, and asked once. The
    /// installer runs again on every update, so asking there would ask
    /// forever; and most people already have a program for demos, so the
    /// question is a nudge, not a setup step. Settings keeps the switch
    /// afterwards either way.
    #[serde(default)]
    pub demo_assoc_asked: bool,
}

/// A named engine launch configuration the user defined in developer
/// mode. `id` is a stable client-generated key for list rendering / edits;
/// `name` is the button label; `args` is the shell-style argument string.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchProfile {
    pub id: String,
    pub name: String,
    pub args: String,
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
            deep_link_auto_connect: false,
            onboarding_completed: false,
            config_version: None,
            developer_mode: false,
            custom_launch_args: String::new(),
            launch_profiles: Vec::new(),
            comps_mode: crate::comps::CompsMode::default(),
            comps_intro_seen: false,
            demo_assoc_asked: false,
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

/// Validate that `demos` is the engine's `demos` folder (or a subfolder of it).
/// The embedded demo player and the `defrag://`-aware bits need the demo to sit
/// at `<install>/<game>/demos/...` so they can derive fs_basepath/fs_game; if the
/// user points the watcher at some unrelated folder outside the engine, none of
/// that works. We require the chosen folder to (1) live inside the engine's
/// install dir (the engine binary's folder) and (2) be - or be inside - a
/// `demos` folder. Returns Ok(()) or a user-facing error string.
pub fn validate_demos_path(engine: &Path, demos: &Path) -> Result<(), String> {
    // We deliberately do NOT require the demos folder to sit under the engine
    // binary's install dir. On Linux the engine lives in one place (often a
    // read-only system path) while all user content - configs, pk3s and demos -
    // lives under the home path (~/.q3a/<game>/demos). Tying demos to the engine
    // dir broke that setup, so instead we validate the folder's *shape*: it must
    // be (inside) a `demos` folder that itself sits in a <base>/<game>/demos
    // layout, which is exactly what the player derives fs_basepath / fs_game
    // from at launch (see demo_player::derive_demo_launch).
    let _ = engine;
    let demos_c = std::fs::canonicalize(demos).unwrap_or_else(|_| demos.to_path_buf());

    // Nearest ancestor named "demos" (the folder itself counts).
    let demos_dir = demos_c
        .ancestors()
        .find(|a| a.file_name().map_or(false, |n| n.eq_ignore_ascii_case("demos")));
    let demos_dir = match demos_dir {
        Some(d) => d,
        None => {
            return Err("Pick your Defrag \"demos\" folder (or a subfolder inside it).".to_string())
        }
    };

    // It must have a parent (the <game> folder) and a grandparent (the install
    // base), or the engine can't resolve fs_game / fs_basepath from it.
    let game = demos_dir.parent();
    if game.and_then(|g| g.parent()).is_none() {
        return Err(
            "That \"demos\" folder isn't inside a game folder - it should look like \
             …/<game>/demos (for example …/defrag/demos)."
                .to_string(),
        );
    }
    Ok(())
}
