//! Tauri commands exposed to the Vue frontend.
//!
//! Each command is a thin wrapper around one of the core modules. Errors
//! collapse to `String` because Tauri's IPC has no structured-error
//! support - the frontend gets a human-readable message and shows it in a
//! toast.

use crate::cache::UploadCache;
use crate::config::{self, Config};
use crate::engine::{self, EngineCandidate};
use crate::history::{ConnectionEntry, ConnectionHistory};
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
    /// Connection history (defrag:// log). Lives in AppState so a
    /// single load happens at boot and subsequent log() calls touch
    /// the same in-memory copy.
    pub history: Arc<ConnectionHistory>,
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
            history: Arc::new(ConnectionHistory::default()),
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

/// Append a message to startup.log. Surfaced to the frontend so the
/// updater can leave a breadcrumb trail of "check started / no update /
/// found vX.Y.Z / network error" in the same log file as Rust-side
/// startup events. Users can then check %APPDATA%\defrag\launcher\
/// startup.log to see exactly when the launcher tried to update and
/// what came back, without needing DevTools.
#[tauri::command]
pub fn log_to_file(msg: String) {
    crate::log_startup(&msg);
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

/// One row in the demo-library view. Mix of filesystem-derived
/// (filename, size, mtime) and cache-derived (hash, demo_id) data so
/// the frontend can show "this one is uploaded as demo #123" without
/// hitting the server. Demos that have never been processed return
/// hash=None and demo_id=None.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DemoLibraryEntry {
    pub path: String,
    pub filename: String,
    pub size_bytes: u64,
    /// Unix-epoch seconds of mtime - frontend renders this with the
    /// user's locale; backend doesn't care about formatting.
    pub mtime: u64,
    pub hash: Option<String>,
    pub demo_id: Option<u64>,
    /// Status from uploaded.json - "done" / "duplicate" - so the
    /// frontend can show "already on defrag.racing" without
    /// re-fetching. None when the file isn't in the cache yet.
    pub upload_status: Option<String>,
}

/// List every .dm_* file in the configured demos folder with the
/// metadata the Library view needs. Reads the FS directly (cheap
/// stat() per file) and joins with uploaded.json for known hashes
/// AND the live queue for known statuses. The queue fallback covers
/// the case where uploaded.json got wiped (corruption, manual delete,
/// pre-bugfix old build) but queue.json still has the user's full
/// processing history - without it, every row would say NOT UPLOADED
/// even though the server has the demo. Hash stays None for
/// queue-only entries so the Render button correctly disables until
/// a fresh hash lands in cache (only the auto-uploader can do that
/// because it has the raw file).
#[tauri::command]
pub fn list_demos(state: State<'_, AppState>) -> Result<Vec<DemoLibraryEntry>, String> {
    use crate::watcher::UploadStatus;
    use std::collections::HashMap;
    use std::time::UNIX_EPOCH;
    let cfg = Config::load().map_err(err_to_string)?;
    let demos = cfg
        .demos_path
        .clone()
        .ok_or_else(|| "Demos path is not set".to_string())?;
    if !demos.is_dir() {
        return Err(format!("Demos path {:?} does not exist", demos));
    }
    let cache = UploadCache::load();

    // Snapshot the live queue and index it by normalised path so the
    // walkdir output (with or without \\?\ prefix) hits the same bucket
    // regardless of how the watcher captured the path originally.
    let queue_snapshot = state.upload_state.snapshot();
    let queue_index: HashMap<PathBuf, (UploadStatus, Option<u64>)> = queue_snapshot
        .items
        .iter()
        .map(|i| (crate::cache::normalize(&i.path), (i.status, i.demo_id)))
        .collect();

    let mut entries = Vec::new();
    let walker: Box<dyn Iterator<Item = PathBuf>> = if cfg.include_subfolders {
        Box::new(
            walkdir::WalkDir::new(&demos)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .map(|e| e.path().to_path_buf()),
        )
    } else {
        Box::new(
            std::fs::read_dir(&demos)
                .map_err(err_to_string)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file()),
        )
    };

    for p in walker {
        let ext_ok = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase().starts_with("dm_"))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let in_temp = p.components().any(|c| {
            c.as_os_str()
                .to_str()
                .map(|s| s.eq_ignore_ascii_case("temp"))
                .unwrap_or(false)
        });
        if in_temp {
            continue;
        }
        let meta = match std::fs::metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let filename = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let cached = cache.get(&p);
        let queue_hit = if cached.is_none() {
            queue_index.get(&crate::cache::normalize(&p))
        } else {
            None
        };
        let upload_status = cached.map(|c| c.status.clone()).or_else(|| {
            queue_hit.and_then(|(s, _)| match s {
                UploadStatus::Done => Some("done".to_string()),
                UploadStatus::Duplicate => Some("duplicate".to_string()),
                _ => None,
            })
        });
        entries.push(DemoLibraryEntry {
            path: p.to_string_lossy().into_owned(),
            filename,
            size_bytes: meta.len(),
            mtime,
            hash: cached.map(|c| c.hash.clone()),
            demo_id: cached
                .and_then(|c| c.demo_id)
                .or_else(|| queue_hit.and_then(|(_, id)| *id)),
            upload_status,
        });
    }

    entries.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    Ok(entries)
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

/// Connect immediately to a URL - used by the Servers tab Connect
/// button, the History tab Reconnect button, and any other in-launcher
/// "join this server" path. The user already saw what they're joining
/// in the UI that called this, so no confirmation banner. Deep-link
/// URLs (forum links etc.) do NOT go through this; they queue via
/// `pending_deep_link` so the user gets a confirmation modal before
/// the engine spawns.
///
/// `enrichment` and `source` get logged to history.json so the History
/// tab can show map + server name alongside the IP, and tell apart
/// "joined from Servers tab" vs "reconnected from History" vs "auto-
/// connected from a defrag:// link". Source defaults to "servers" so
/// the common Servers-tab call site doesn't have to pass anything.
#[tauri::command]
pub fn handle_protocol_url(
    state: State<'_, AppState>,
    url: String,
    enrichment: Option<ConnectEnrichment>,
    source: Option<String>,
) -> Result<String, String> {
    let addr = protocol::parse_url(&url).map_err(err_to_string)?;
    let cfg = Config::load().map_err(err_to_string)?;
    protocol::launch(cfg.engine_path.as_deref(), &addr).map_err(err_to_string)?;
    let enrich = enrichment.unwrap_or_default();
    state.history.log(
        addr.host().to_string(),
        addr.port(),
        enrich.map,
        enrich.server_name,
        enrich.physics,
        source.as_deref().unwrap_or("servers"),
    );
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

/// Notifications feed for the Notifications tab. Token-locked.
/// Returns the raw shape /api/launcher/notifications produces:
///   { records: [...], system: [...], unread: {records, system, total} }
#[tauri::command]
pub async fn get_notifications() -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.fetch_notifications().await.map_err(err_to_string)
}

/// Bell-badge-only fetch. Returns `{records, system, total}`. The App
/// bell poll calls this every 180s instead of pulling the full feed.
#[tauri::command]
pub async fn get_notifications_unread_count() -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.fetch_notifications_unread_count().await.map_err(err_to_string)
}

/// Queue a YouTube render for a local demo. Caller passes the file
/// hash from uploaded.json. Returns the server's response unchanged
/// so the frontend can react to all four cases (queued / already
/// queued / needs upload / quota exhausted) without us mapping them
/// into a single Rust type. status is 200/404/422/429 and the body
/// carries the detail.
#[tauri::command]
pub async fn request_render(file_hash: String) -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    let resp = client.request_render(&file_hash).await.map_err(err_to_string)?;
    let status = resp.status().as_u16();
    // Pass the body through with an added _http_status field so the
    // frontend can branch on "200 vs 404 needs_upload" cleanly. We
    // unwrap the body via .json::<Value>() unconditionally because
    // the controller always returns JSON for all status codes.
    let mut body = resp
        .json::<serde_json::Value>()
        .await
        .map_err(err_to_string)?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("_http_status".into(), serde_json::Value::from(status));
    }
    Ok(body)
}

/// Per-row toggle / bulk mark-as-read mutations for the Notifications
/// tab. Each returns the server's response unchanged (always carries
/// fresh `unread` counts) so the frontend can update the bell badge
/// without a separate /notifications round-trip.
#[tauri::command]
pub async fn notification_record_toggle(id: u64) -> Result<serde_json::Value, String> {
    let token = token::load().map_err(err_to_string)?.ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.notification_record_toggle(id).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn notification_records_mark_read() -> Result<serde_json::Value, String> {
    let token = token::load().map_err(err_to_string)?.ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.notification_records_mark_read().await.map_err(err_to_string)
}

#[tauri::command]
pub async fn notification_records_mark_unread() -> Result<serde_json::Value, String> {
    let token = token::load().map_err(err_to_string)?.ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.notification_records_mark_unread().await.map_err(err_to_string)
}

#[tauri::command]
pub async fn notification_system_toggle(id: u64) -> Result<serde_json::Value, String> {
    let token = token::load().map_err(err_to_string)?.ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.notification_system_toggle(id).await.map_err(err_to_string)
}

#[tauri::command]
pub async fn notification_system_mark_read() -> Result<serde_json::Value, String> {
    let token = token::load().map_err(err_to_string)?.ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.notification_system_mark_read().await.map_err(err_to_string)
}

#[tauri::command]
pub async fn notification_system_mark_unread() -> Result<serde_json::Value, String> {
    let token = token::load().map_err(err_to_string)?.ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.notification_system_mark_unread().await.map_err(err_to_string)
}

/// Cheap polling endpoint for a single demo's current render state.
/// Returns the server's response verbatim (has_render bool + status
/// + youtube_url when present). Used by the Library view to keep
/// row states fresh without reloading the full notifications feed.
#[tauri::command]
pub async fn get_render_status(file_hash: String) -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.render_status(&file_hash).await.map_err(err_to_string)
}

/// "Who am I" lookup for the Profile button. Token-locked. Frontend
/// calls this once at app boot and caches the result so the button
/// works without a re-fetch on every render.
#[tauri::command]
pub async fn get_me() -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.fetch_me().await.map_err(err_to_string)
}

/// Records browser feed for the Records tab. Token-locked,
/// launcher:read ability required. Physics is one of "vq3" / "cpm",
/// page is 1-based. Returns the raw Laravel paginator JSON; the
/// frontend reads `data`, `current_page`, `last_page`, `total`.
#[tauri::command]
pub async fn get_records(physics: String, page: u32) -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved - records browser requires a launcher token from defrag.racing/user/settings".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.fetch_records(&physics, page).await.map_err(err_to_string)
}

/// Maps browser feed for the Maps tab. Same token/ability shape as
/// get_records. Search is an optional substring matched against map
/// name (case-insensitive on the SQL side).
#[tauri::command]
pub async fn get_maps(page: u32, search: Option<String>) -> Result<serde_json::Value, String> {
    let token = token::load()
        .map_err(err_to_string)?
        .ok_or_else(|| "No token saved - maps browser requires a launcher token from defrag.racing/user/settings".to_string())?;
    let client = crate::api::Client::new(config::api_base_url(), token).map_err(err_to_string)?;
    client.fetch_maps(page, search.as_deref()).await.map_err(err_to_string)
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
///
/// `enrichment` carries the optional server detail (map / name /
/// physics) that the frontend looked up while the banner was visible -
/// we log it to history.json so the History tab can show "joined
/// ^1EU CPM I on bug22_slick" instead of just an IP. Auto-connect
/// paths log via a separate code path with whatever they have, which
/// for now is just IP:port.
#[derive(Debug, serde::Deserialize, Default)]
pub struct ConnectEnrichment {
    pub map: Option<String>,
    pub server_name: Option<String>,
    pub physics: Option<String>,
}

#[tauri::command]
pub fn confirm_pending_deep_link(
    app: AppHandle,
    state: State<'_, AppState>,
    enrichment: Option<ConnectEnrichment>,
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
    protocol::launch(cfg.engine_path.as_deref(), &addr).map_err(err_to_string)?;
    let enrich = enrichment.unwrap_or_default();
    state.history.log(
        addr.host().to_string(),
        addr.port(),
        enrich.map,
        enrich.server_name,
        enrich.physics,
        "confirmed",
    );
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

// ---- Connection history ----------------------------------------------------

/// Snapshot of the defrag:// connection log for the History tab.
/// Returns at most HISTORY_CAP entries, newest first.
#[tauri::command]
pub fn get_connection_history(state: State<'_, AppState>) -> Vec<ConnectionEntry> {
    state.history.list()
}

/// Wipe history from memory and disk. Surfaced via the "Clear" button
/// in the History tab.
#[tauri::command]
pub fn clear_connection_history(state: State<'_, AppState>) {
    state.history.clear();
}

// ---- Per-row Library / Dashboard actions ----------------------------------

/// Re-queue a single demo for upload after it landed in Error status.
/// Sends Message::FileAdded which handle_file then re-processes - the
/// already_present check only short-circuits Done/Duplicate, so Error
/// rows naturally re-enter the hash + upload path. No-op when the
/// watcher isn't running (the row would have nowhere to be processed);
/// the Dashboard hides the Retry button in that case anyway.
#[tauri::command]
pub fn retry_upload(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    let handle_guard = state.watcher.lock().unwrap();
    let h = handle_guard
        .as_ref()
        .ok_or_else(|| "Auto-upload is off - press Start to enable retries".to_string())?;
    h.tx.send(Message::FileAdded(path_buf))
        .map_err(|e| format!("queue full: {}", e))
}

/// Delete a demo file from disk and forget its cache row. Used by the
/// Library context menu. If the running watcher has a queue entry for
/// the same path, we drop that too so the row disappears from the
/// Dashboard without a refresh.
///
/// Deliberately conservative about the path: must be a file (not a
/// directory) and must exist - any error other than "already gone" is
/// surfaced to the user. The cache + queue wipes are best-effort.
#[tauri::command]
pub fn delete_demo(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.exists() {
        // Tolerate: pretend the delete succeeded so the UI row goes
        // away without a confusing error if the user already nuked the
        // file from explorer.
        let mut cache = UploadCache::load();
        cache.remove(&path_buf);
        let _ = cache.save();
        state.upload_state.remove_path(&path_buf);
        return Ok(());
    }
    if !path_buf.is_file() {
        return Err(format!("Not a file: {}", path));
    }
    std::fs::remove_file(&path_buf).map_err(err_to_string)?;
    let mut cache = UploadCache::load();
    cache.remove(&path_buf);
    let _ = cache.save();
    state.upload_state.remove_path(&path_buf);
    Ok(())
}
