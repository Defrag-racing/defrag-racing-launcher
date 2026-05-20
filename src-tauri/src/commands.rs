//! Tauri commands exposed to the Vue frontend.
//!
//! Each command is a thin wrapper around one of the core modules. Errors
//! collapse to `String` because Tauri's IPC has no structured-error
//! support — the frontend gets a human-readable message and shows it in a
//! toast.

use crate::config::{self, Config};
use crate::engine::{self, EngineCandidate};
use crate::protocol;
use crate::token;
use crate::watcher::{self, UploadStateSnapshot, WatcherHandle};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, State};

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
    let cfg = Config::load().map_err(err_to_string)?;
    let demos = cfg
        .demos_path
        .clone()
        .ok_or_else(|| "Demos path is not set".to_string())?;
    if !demos.is_dir() {
        return Err(format!("Demos path {:?} does not exist", demos));
    }
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved — generate one at defrag.racing and paste it in settings".to_string())?;

    let handle = watcher::start(app, demos, config::api_base_url(), token)
        .map_err(err_to_string)?;

    *state.watcher.lock().unwrap() = Some(handle);
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
