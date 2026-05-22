//! Tauri commands exposed to the Vue frontend.
//!
//! Each command is a thin wrapper around one of the core modules. Errors
//! collapse to `String` because Tauri's IPC has no structured-error
//! support — the frontend gets a human-readable message and shows it in a
//! toast.

use crate::cache::UploadCache;
use crate::config::{self, Config};
use crate::engine::{self, EngineCandidate};
use crate::protocol;
use crate::token;
use crate::watcher::{self, UploadStateSnapshot, WatcherHandle};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

/// Shared app state — the current watcher (if running) and a cached config.
/// Swapped out when the user changes demos path or rotates the token.
pub struct AppState {
    pub watcher: Mutex<Option<WatcherHandle>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            watcher: Mutex::new(None),
        }
    }
}

fn err_to_string<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---- Config ----------------------------------------------------------------

#[tauri::command]
pub fn get_config() -> Result<Config, String> {
    Config::load().map_err(err_to_string)
}

#[tauri::command]
pub fn save_config(cfg: Config) -> Result<(), String> {
    cfg.save().map_err(err_to_string)
}

#[tauri::command]
pub fn complete_onboarding() -> Result<(), String> {
    let mut cfg = Config::load().map_err(err_to_string)?;
    cfg.onboarding_completed = true;
    cfg.save().map_err(err_to_string)?;
    Ok(())
}

/// Returns the launcher version the persisted config was last written by,
/// but only when it's different from the current running version. Used by
/// the UI to show a one-time "Previous install detected — start fresh or
/// keep settings?" screen on boot.
#[tauri::command]
pub fn previous_version() -> Result<Option<String>, String> {
    let cfg = Config::load().map_err(err_to_string)?;
    Ok(config::previous_version(&cfg))
}

/// Stamp the persisted config with the current launcher version. Called
/// by the UI after the user dismisses the version-mismatch dialog with
/// "Keep settings".
#[tauri::command]
pub fn acknowledge_version() -> Result<(), String> {
    let cfg = Config::load().map_err(err_to_string)?;
    cfg.save().map_err(err_to_string)?;
    Ok(())
}

/// Version string of the running launcher — frontend uses it to render
/// "v0.1.3" consistently without hard-coding in Vue files.
#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ---- Token -----------------------------------------------------------------

#[tauri::command]
pub fn save_token(token: String) -> Result<(), String> {
    token::save(&token).map_err(err_to_string)
}

#[tauri::command]
pub fn has_token() -> Result<bool, String> {
    Ok(token::load().map_err(err_to_string)?.is_some())
}

#[tauri::command]
pub fn clear_token() -> Result<(), String> {
    token::clear().map_err(err_to_string)
}

/// Wipe every bit of persistent state the launcher owns: token, config,
/// and the running watcher. Used when the user clicks "Reset launcher" in
/// settings so they can start over without uninstalling. Does NOT touch
/// the demos folder itself.
#[tauri::command]
pub fn reset_launcher(state: State<'_, AppState>) -> Result<(), String> {
    *state.watcher.lock().unwrap() = None;
    let _ = token::clear();
    if let Ok(path) = Config::path() {
        let _ = std::fs::remove_file(path);
    }
    // Cache is owned by us — wipe it too so a reset really does start
    // from zero (otherwise re-onboarded user with new token would skip
    // re-uploading demos the prior token already covered).
    let _ = UploadCache::clear();
    Ok(())
}

/// Force the next rescan to re-hash every file and re-query the server,
/// regardless of cache state. Used when an admin deleted a demo on the
/// server and the user wants to re-upload, or when the user suspects
/// the cache has drifted.
#[tauri::command]
pub fn clear_upload_cache() -> Result<(), String> {
    UploadCache::clear().map_err(err_to_string)
}

// ---- Autostart -------------------------------------------------------------

/// Returns whether the OS has the launcher registered to autostart on
/// login. Reflects current OS state, not just our Settings UI — if the
/// user removed it manually (e.g. via Task Manager → Startup) we'll
/// pick that up next time the toggle is read.
#[tauri::command]
pub fn is_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(err_to_string)
}

/// Flip the OS-level autostart registration. Plugin writes to Windows
/// Run reg key / macOS LaunchAgent plist / Linux ~/.config/autostart/
/// .desktop depending on platform. The HIDDEN_FLAG configured in lib.rs
/// is included automatically by the plugin so the next autostart launch
/// starts to the tray.
#[tauri::command]
pub fn set_autostart_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(err_to_string)?;
    } else {
        mgr.disable().map_err(err_to_string)?;
    }
    Ok(())
}

// ---- Engine detection ------------------------------------------------------

#[tauri::command]
pub fn detect_engines() -> Vec<EngineCandidate> {
    engine::detect()
}

#[tauri::command]
pub fn guess_demos_path(engine_path: PathBuf) -> Option<PathBuf> {
    config::guess_demos_path_from_engine(&engine_path)
}

// ---- Watcher ---------------------------------------------------------------

#[tauri::command]
pub fn start_auto_upload(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::log_startup("start_auto_upload: entry");
    let cfg = Config::load().map_err(err_to_string)?;
    crate::log_startup(&format!(
        "start_auto_upload: cfg loaded demos_path={:?} include_subfolders={}",
        cfg.demos_path, cfg.include_subfolders
    ));
    let demos = cfg
        .demos_path
        .clone()
        .ok_or_else(|| "Demos path is not set".to_string())?;
    if !demos.is_dir() {
        crate::log_startup(&format!("start_auto_upload: demos not a dir: {:?}", demos));
        return Err(format!("Demos path {:?} does not exist", demos));
    }
    crate::log_startup("start_auto_upload: demos dir ok, loading token");
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved — generate one at defrag.racing and paste it in settings".to_string())?;
    crate::log_startup(&format!("start_auto_upload: token loaded ({} bytes)", token.len()));

    crate::log_startup("start_auto_upload: calling watcher::start");
    let handle = watcher::start(
        app,
        demos,
        cfg.include_subfolders,
        config::api_base_url(),
        token,
    )
    .map_err(err_to_string)?;
    crate::log_startup("start_auto_upload: watcher::start returned ok");

    *state.watcher.lock().unwrap() = Some(handle);
    crate::log_startup("start_auto_upload: stored handle, returning");
    Ok(())
}

#[tauri::command]
pub fn stop_auto_upload(state: State<'_, AppState>) -> Result<(), String> {
    // Dropping the handle stops the debouncer + cancels the worker task.
    *state.watcher.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
pub fn is_auto_upload_running(state: State<'_, AppState>) -> bool {
    state.watcher.lock().unwrap().is_some()
}

#[tauri::command]
pub fn get_upload_state(state: State<'_, AppState>) -> UploadStateSnapshot {
    match state.watcher.lock().unwrap().as_ref() {
        Some(h) => h.state.snapshot(),
        None => UploadStateSnapshot::default(),
    }
}

// ---- defrag:// protocol -----------------------------------------------------

/// Handle a `defrag://<ip>:<port>` deep link: validate the URL, look up
/// the configured engine, and spawn it with `+connect <ip>:<port>`.
///
/// Called both from the Rust deep-link plugin handler (when the link
/// fires the launcher externally) and directly from the UI (e.g. a
/// "Connect" button on the dashboard that takes a manually-entered ip).
/// Returns the parsed address as a string so the UI can show a toast
/// like "Connecting to 1.2.3.4:27960…".
#[tauri::command]
pub fn handle_protocol_url(url: String) -> Result<String, String> {
    let addr = protocol::parse_url(&url).map_err(err_to_string)?;
    let cfg = Config::load().map_err(err_to_string)?;
    protocol::launch(cfg.engine_path.as_deref(), addr).map_err(err_to_string)?;
    Ok(addr.to_string())
}
