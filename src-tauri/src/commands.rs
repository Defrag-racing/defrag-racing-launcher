//! Tauri commands exposed to the Vue frontend.
//!
//! Each command is a thin wrapper around one of the core modules. Errors
//! collapse to `String` because Tauri's IPC has no structured-error
//! support - the frontend gets a human-readable message and shows it in a
//! toast.

use crate::cache::UploadCache;
use crate::config::{self, Config};
use crate::engine::{self, EngineCandidate};
use crate::protocol;
use crate::token;
use crate::watcher::{self, Message, UploadState, UploadStateSnapshot, WatcherHandle};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

/// Shared app state - the current watcher (if running) plus the most
/// recently received defrag:// URL waiting for user confirmation. The
/// pending URL is kept in backend state (not just frontend) so a cold
/// start via defrag:// can hand off the URL to the webview once it
/// mounts - the deep-link event fires before the frontend exists.
///
/// `upload_state` lives independently of the watcher so the activity
/// feed survives Stop+Start (and full app restart - it's loaded from
/// queue.json at boot). The watcher borrows the same Arc while running
/// and saves it to disk on its way out.
pub struct AppState {
    pub watcher: Mutex<Option<WatcherHandle>>,
    pub upload_state: Arc<UploadState>,
    pub pending_deep_link: Mutex<Option<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        let upload_state = Arc::new(UploadState::default());
        // Pull persisted queue items into memory before the webview
        // mounts so the Dashboard's first get_upload_state poll shows
        // history instead of an empty list.
        upload_state.load_persisted();
        Self {
            watcher: Mutex::new(None),
            upload_state,
            pending_deep_link: Mutex::new(None),
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
/// the UI to show a one-time "Previous install detected - start fresh or
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

/// Version string of the running launcher - frontend uses it to render
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
    // Cache is owned by us - wipe it too so a reset really does start
    // from zero (otherwise re-onboarded user with new token would skip
    // re-uploading demos the prior token already covered).
    let _ = UploadCache::clear();
    // Persisted queue history - blank the Dashboard so a re-onboarded
    // user doesn't see stale rows from the previous account.
    let _ = UploadState::clear_persisted();
    // Wipe the in-memory queue too so the UI updates immediately
    // without waiting for a restart.
    state.upload_state.clear_items();
    Ok(())
}

/// "Force re-check" in Settings - means exactly what it says on the
/// label: forget every uploaded-hash record and re-process the entire
/// demos folder from scratch. Three coordinated wipes + an immediate
/// rescan are needed because the cache, the queue items, and the
/// already_present early-return in handle_file all independently keep
/// state that would otherwise cause "Force re-check" to silently do
/// nothing:
///
///   1. uploaded.json - the size+mtime → hash cache. If this isn't
///      cleared, every file hits the cache on the next rescan and is
///      marked Duplicate without a server call.
///   2. queue.json + in-memory items. Even with cache empty,
///      handle_file's early-return skips any path already in the
///      visible queue with Done/Duplicate status, so without this
///      wipe a returning user with 5000 cached queue rows would see
///      no re-checks happen at all.
///   3. Immediate RescanFolder kick. The user pressed the button to
///      see something happen; we shouldn't make them Stop+Start to
///      trigger the rescan they just asked for. No-op when the
///      watcher isn't running - the next Start will rescan anyway.
#[tauri::command]
pub fn clear_upload_cache(state: State<'_, AppState>) -> Result<(), String> {
    UploadCache::clear().map_err(err_to_string)?;
    let _ = watcher::UploadState::clear_persisted();
    state.upload_state.clear_items();
    if let Some(h) = state.watcher.lock().unwrap().as_ref() {
        let cfg = Config::load().map_err(err_to_string)?;
        if let Some(demos) = cfg.demos_path {
            let _ = h.tx.send(watcher::Message::RescanFolder {
                folder: demos,
                recursive: cfg.include_subfolders,
            });
        }
    }
    Ok(())
}

// ---- Autostart -------------------------------------------------------------

/// Returns whether the OS has the launcher registered to autostart on
/// login. Reflects current OS state, not just our Settings UI - if the
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
        .ok_or_else(|| "No token saved - generate one at defrag.racing and paste it in settings".to_string())?;
    crate::log_startup(&format!("start_auto_upload: token loaded ({} bytes)", token.len()));

    crate::log_startup("start_auto_upload: calling watcher::start");
    let handle = watcher::start(
        app,
        state.upload_state.clone(),
        demos,
        cfg.include_subfolders,
        config::api_base_url(),
        token,
        cfg.cpu_throttle_pct,
    )
    .map_err(err_to_string)?;
    crate::log_startup("start_auto_upload: watcher::start returned ok");

    *state.watcher.lock().unwrap() = Some(handle);
    // Stick the "I want auto-upload on" preference so the next launcher
    // boot brings the watcher up automatically. The user clicked Start;
    // they shouldn't have to click it again every cold launch. We
    // persist after the watcher actually started so a Start that fails
    // (no token, demos folder gone) doesn't poison the next launch.
    if !cfg.auto_upload_enabled {
        let mut updated = cfg;
        updated.auto_upload_enabled = true;
        if let Err(e) = updated.save() {
            crate::log_startup(&format!(
                "start_auto_upload: failed to persist auto_upload_enabled: {}",
                e
            ));
        }
    }
    crate::log_startup("start_auto_upload: stored handle, returning");
    Ok(())
}

#[tauri::command]
pub fn stop_auto_upload(state: State<'_, AppState>) -> Result<(), String> {
    // Dropping the handle stops the debouncer + cancels the worker task.
    *state.watcher.lock().unwrap() = None;
    // Symmetric with start: an explicit Stop unsticks the preference so
    // the next boot stays quiet. Pause is a separate thing (worker
    // parks, watcher stays alive) and doesn't touch this flag.
    if let Ok(cfg) = Config::load() {
        if cfg.auto_upload_enabled {
            let mut updated = cfg;
            updated.auto_upload_enabled = false;
            let _ = updated.save();
        }
    }
    Ok(())
}

/// Pause the worker but keep the filesystem watcher running. New demos
/// keep accumulating in the queue; the worker resumes processing on
/// `resume_auto_upload`. Lets the user halt launcher activity (during a
/// race, while on a metered connection, etc.) without losing demos that
/// get recorded in the meantime - a full stop would drop the debouncer
/// and miss them.
#[tauri::command]
pub fn pause_auto_upload(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(h) = state.watcher.lock().unwrap().as_ref() {
        h.state.set_paused(true);
    }
    Ok(())
}

#[tauri::command]
pub fn resume_auto_upload(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(h) = state.watcher.lock().unwrap().as_ref() {
        h.state.set_paused(false);
        // Kick the worker to re-process anything stuck in Pending -
        // most commonly, a file whose hashing pass was aborted by the
        // pause. Without this, those rows would sit there waiting for
        // an unrelated filesystem event to wake the queue.
        let _ = h.tx.send(Message::RedrivePending);
    }
    Ok(())
}

#[tauri::command]
pub fn is_auto_upload_running(state: State<'_, AppState>) -> bool {
    state.watcher.lock().unwrap().is_some()
}

#[tauri::command]
pub fn is_auto_upload_paused(state: State<'_, AppState>) -> bool {
    state
        .watcher
        .lock()
        .unwrap()
        .as_ref()
        .map_or(false, |h| h.state.is_paused())
}

/// Unix-epoch ms at which the active 429 backoff ends, or 0 when not
/// rate-limited. Dashboard polls this once a second and renders a
/// countdown banner while > now.
#[tauri::command]
pub fn get_rate_limit_resume_at_ms(state: State<'_, AppState>) -> u64 {
    state
        .watcher
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0, |h| h.state.rate_limit_resume_at_ms())
}

/// Current CPU-throttle target the running watcher is using. Falls back
/// to the persisted config value when no watcher is up - UI uses this
/// to show the live percentage on the Speed-up button.
#[tauri::command]
pub fn get_cpu_throttle_pct(state: State<'_, AppState>) -> Result<u8, String> {
    if let Some(h) = state.watcher.lock().unwrap().as_ref() {
        return Ok(h.state.cpu_throttle_pct());
    }
    let cfg = Config::load().map_err(err_to_string)?;
    Ok(cfg.cpu_throttle_pct)
}

/// Live-update the throttle target. Takes effect on the very next
/// post-hash sleep - no watcher restart needed. Does NOT persist to
/// config; the Speed-up button on Dashboard uses this to temporarily
/// override the user's saved preference while a big rescan drains,
/// without rewriting their preferred-default setting on disk.
#[tauri::command]
pub fn set_cpu_throttle_pct_runtime(state: State<'_, AppState>, pct: u8) {
    if let Some(h) = state.watcher.lock().unwrap().as_ref() {
        h.state.set_cpu_throttle_pct(pct);
    }
}

#[tauri::command]
pub fn get_upload_state(state: State<'_, AppState>) -> UploadStateSnapshot {
    // Read straight off AppState's shared UploadState - the watcher
    // (when running) writes to this same Arc, and on cold start it's
    // populated from queue.json by AppState::default(). Means the
    // Dashboard sees its history on first mount even before Start is
    // pressed.
    state.upload_state.snapshot()
}

// ---- defrag:// protocol -----------------------------------------------------

/// Connect immediately to a URL - used for manually-entered IPs from a
/// "Quick connect" UI where the user is *typing* the address (so the
/// click on Connect is already their confirmation). Deep-link URLs do
/// NOT go through this; they queue via `pending_deep_link` so the user
/// gets a confirmation button before the engine spawns.
#[tauri::command]
pub fn handle_protocol_url(url: String) -> Result<String, String> {
    let addr = protocol::parse_url(&url).map_err(err_to_string)?;
    let cfg = Config::load().map_err(err_to_string)?;
    protocol::launch(cfg.engine_path.as_deref(), addr).map_err(err_to_string)?;
    Ok(addr.to_string())
}

/// Launch the configured engine at the main menu (no +connect). One-click
/// "play the game" from the Dashboard for tray-resident users who don't
/// want to dig the engine binary out of their filesystem.
#[tauri::command]
pub fn launch_engine() -> Result<(), String> {
    let cfg = Config::load().map_err(err_to_string)?;
    protocol::launch_no_connect(cfg.engine_path.as_deref()).map_err(err_to_string)
}

// ---- Server browser --------------------------------------------------------

/// Fetch the live server list from defrag.racing. Token-locked endpoint -
/// requires a launcher token with the `launcher:read` ability. The
/// returned JSON is the same shape the website /api/servers/live serves
/// (via the shared ServerListService) plus per-user mytime/myrank
/// fields populated for the token's owner. Frontend renders directly.
#[tauri::command]
pub async fn get_servers() -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved - server browser requires a launcher token from defrag.racing/user/settings".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.fetch_servers().await.map_err(err_to_string)
}

/// Read (without consuming) the URL waiting for user confirmation. Called
/// by the frontend on mount so cold-start-via-deep-link surfaces the
/// Connect prompt without the user needing to re-click the link.
#[tauri::command]
pub fn get_pending_deep_link(state: State<'_, AppState>) -> Option<String> {
    state.pending_deep_link.lock().unwrap().clone()
}

/// User clicked Connect on the pending-URL banner: take the URL, parse,
/// and actually launch the engine. The Mutex `take()` clears pending so
/// the banner disappears and the URL can't be replayed.
#[tauri::command]
pub fn confirm_pending_deep_link(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    use tauri::Manager;
    let url = state
        .pending_deep_link
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "No pending connection".to_string())?;
    let addr = protocol::parse_url(&url).map_err(err_to_string)?;
    let cfg = Config::load().map_err(err_to_string)?;
    protocol::launch(cfg.engine_path.as_deref(), addr).map_err(err_to_string)?;
    // Engine has focus now - drop the launcher back to the tray so it
    // isn't pointlessly floating over Quake.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    Ok(addr.to_string())
}

/// User clicked Dismiss on the pending-URL banner: just drop it.
#[tauri::command]
pub fn cancel_pending_deep_link(state: State<'_, AppState>) {
    *state.pending_deep_link.lock().unwrap() = None;
}
