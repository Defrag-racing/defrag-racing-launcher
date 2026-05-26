mod api;
mod cache;
mod commands;
mod config;
mod engine;
mod hashing;
mod history;
mod protocol;
mod token;
mod watcher;

use commands::AppState;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::MacosLauncher;

/// CLI flag passed by the autostart plugin when the OS launches us at
/// login. Lets us start with the window hidden (tray-only) so the user
/// isn't ambushed by a popup every boot - they explicitly clicked
/// "Show" from the tray to see the dashboard.
const HIDDEN_FLAG: &str = "--hidden";

/// Append a line to %APPDATA%\defrag\launcher\startup.log. Used by the
/// init code paths to leave breadcrumbs about how far we got - when
/// the launcher exits silently on first run there's no Event Viewer
/// entry and no CMD output (GUI subsystem swallows stderr), so a file
/// is the only diagnostic surface that survives.
pub fn log_startup(msg: &str) {
    let Some(dirs) = directories::ProjectDirs::from("racing", "defrag", "launcher") else {
        return;
    };
    let dir = dirs.config_dir();
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("startup.log");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", ts, msg);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Panic hook BEFORE anything else - catches panics during plugin
    // init, tray construction, etc. and writes them to startup.log so
    // we have something to look at when the process disappears with
    // no Event Viewer trace.
    std::panic::set_hook(Box::new(|info| {
        log_startup(&format!("PANIC: {}", info));
    }));

    log_startup(&format!(
        "=== launch === version={} argv={:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::args().collect::<Vec<_>>()
    ));

    // Logging is environment-controlled (RUST_LOG=debug) so a shipped
    // binary stays quiet unless the user flips it on while debugging.
    let _ = env_logger::try_init();

    let started_hidden = std::env::args().any(|a| a == HIDDEN_FLAG);
    log_startup(&format!("started_hidden={}", started_hidden));

    let builder = tauri::Builder::default();
    log_startup("Builder::default ok");

    // single-instance MUST be the first plugin registered - when a second
    // launcher process is spawned (e.g. by a defrag:// click while the
    // launcher is already open) this plugin hands the new process's argv
    // to the existing one and exits the new one. Anything later would
    // run in both processes briefly. Shadow `builder` rather than mutate
    // so macOS (where the cfg block is dead code) doesn't trip an
    // unused_mut warning.
    //
    // We deliberately do NOT process the argv URL ourselves here -
    // `tauri-plugin-deep-link` integrates with single-instance and will
    // fire `on_open_url` for the forwarded URL. Doing it manually as
    // well caused the engine to launch twice for one defrag:// click.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        log_startup(&format!("single_instance callback fired argv={:?}", argv));
        // Surface the existing window so the user sees the toast emitted
        // by the deep-link handler.
        show_main_window(app);
    }));
    log_startup("single_instance plugin registered");

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Autostart is opt-in; the plugin only enables itself when the
        // user flips the Settings toggle (which calls plugin.enable()).
        // We pass HIDDEN_FLAG so the autostart-spawned launcher starts
        // in the tray instead of stealing focus on every login.
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(AppState::default())
        .setup(move |app| {
            log_startup("setup() entered");

            // Hide on launch if we were started by the OS at login -
            // user explicitly opted into autostart and expects a quiet
            // background process, not a pop-up.
            if started_hidden {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                    log_startup("hid main window (started_hidden)");
                }
            }

            // Tray icon - keeps the launcher alive after the user
            // closes the window so the demo watcher + defrag:// handler
            // keep doing their job. Without this the process would exit
            // on last-window-close (default Tauri behavior).
            log_startup("calling build_tray");
            build_tray(app)?;
            log_startup("build_tray ok");

            // Register defrag:// scheme at runtime - needed in dev where
            // the bundled installer hasn't registered it with the OS,
            // and as a fallback on Linux distros that don't honour the
            // .desktop file's MimeType immediately.
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register("defrag");

                // Listen for deep-link events fired while the launcher
                // is already running.
                let app_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_deep_link_url(&app_handle, url.as_str());
                    }
                });
            }

            // Auto-resume the watcher on launch if the user's last
            // explicit action was Start. start_auto_upload() flips
            // auto_upload_enabled=true on click, stop_auto_upload()
            // flips it back, so a one-time click means "keep doing
            // this across launches" - the user shouldn't have to
            // re-click Start every cold boot. Best-effort: any failure
            // (missing token, demos folder gone) just logs and lets
            // the user fix it manually from the Dashboard, same as
            // they would on a regular Start click.
            autostart_watcher_if_enabled(app.handle());

            log_startup("setup() complete");
            Ok(())
        })
        .on_window_event(|window, event| {
            // Intercept the OS "close" button: hide instead of destroy
            // so the launcher stays alive in the tray. The user can
            // still quit explicitly via the tray menu's Quit item.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::complete_onboarding,
            commands::previous_version,
            commands::acknowledge_version,
            commands::app_version,
            commands::save_token,
            commands::has_token,
            commands::clear_token,
            commands::reset_launcher,
            commands::detect_engines,
            commands::guess_demos_path,
            commands::start_auto_upload,
            commands::stop_auto_upload,
            commands::pause_auto_upload,
            commands::resume_auto_upload,
            commands::is_auto_upload_running,
            commands::is_auto_upload_paused,
            commands::get_upload_state,
            commands::clear_upload_cache,
            commands::get_cpu_throttle_pct,
            commands::set_cpu_throttle_pct_runtime,
            commands::get_rate_limit_resume_at_ms,
            commands::handle_protocol_url,
            commands::launch_engine,
            commands::get_servers,
            commands::get_pending_deep_link,
            commands::confirm_pending_deep_link,
            commands::cancel_pending_deep_link,
            commands::get_connection_history,
            commands::clear_connection_history,
            commands::get_records,
            commands::get_maps,
            commands::get_me,
            commands::set_autostart_enabled,
            commands::is_autostart_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    log_startup("=== exit ===");
}

/// Build the tray icon, attach a Show / Quit menu, and wire left-click
/// to "Show" so a single click on the tray icon brings the dashboard
/// up without forcing the user to discover the right-click menu first.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    // Prefer the bundle's window icon - Windows packs that into the .exe
    // and Tauri exposes it via default_window_icon(). On a fresh MSI
    // install the icon resource has been observed as not-yet-loaded
    // when setup() runs (0.1.6 crashed with 0xc0000409 on first launch
    // because we unwrap()'d the None). Fall back to a PNG embedded at
    // compile time so tray creation can never fail for "no icon".
    let icon = app
        .default_window_icon()
        .cloned()
        .unwrap_or_else(|| tauri::include_image!("icons/128x128.png"));

    let _tray = TrayIconBuilder::new()
        .icon(icon)
        .tooltip("Defrag Racing Launcher")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click on the icon = show the window. We deliberately
            // don't react to the icon click when the click is the
            // closing-up of a right-click context menu.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Spin up the upload watcher on launch when the persisted config
/// says the user wants it - i.e. their last explicit click was Start
/// rather than Stop. Mirrors what commands::start_auto_upload does,
/// minus the Tauri-command plumbing (we have an AppHandle here, not a
/// State<AppState>). Any failure just logs and leaves the watcher off;
/// the Dashboard will show its normal "click Start" empty state and
/// the user can fix the underlying problem (missing token, gone demos
/// folder) at their leisure.
fn autostart_watcher_if_enabled(app: &tauri::AppHandle) {
    use tauri::Manager;
    let cfg = match config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            log_startup(&format!("autostart: config load failed: {}", e));
            return;
        }
    };
    if !cfg.auto_upload_enabled {
        log_startup("autostart: auto_upload_enabled=false, skipping");
        return;
    }
    let Some(demos) = cfg.demos_path.clone() else {
        log_startup("autostart: no demos_path, skipping");
        return;
    };
    if !demos.is_dir() {
        log_startup(&format!("autostart: demos_path not a dir: {:?}", demos));
        return;
    }
    let token = match token::load() {
        Ok(Some(t)) => t,
        Ok(None) => {
            log_startup("autostart: no token saved, skipping");
            return;
        }
        Err(e) => {
            log_startup(&format!("autostart: token load failed: {}", e));
            return;
        }
    };
    let state: tauri::State<AppState> = app.state();
    let handle = match watcher::start(
        app.clone(),
        state.upload_state.clone(),
        demos,
        cfg.include_subfolders,
        config::api_base_url(),
        token,
        cfg.cpu_throttle_pct,
    ) {
        Ok(h) => h,
        Err(e) => {
            log_startup(&format!("autostart: watcher::start failed: {}", e));
            return;
        }
    };
    *state.watcher.lock().unwrap() = Some(handle);
    log_startup("autostart: watcher started");
}

/// Bring the main window from hidden / minimized into the foreground.
/// Used by tray clicks, single-instance forwarding, and the deep-link
/// handler - anywhere we want the user to actually see the UI.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Single source of truth for what happens when a defrag:// URL arrives -
/// from any path (deep-link plugin event, single-instance argv forwarding,
/// or cold start). We deliberately do **not** auto-launch the engine here;
/// instead the URL is stashed as "pending" and the launcher window pops
/// with a Connect button. Reasons:
///   - prevents an accidental click on a defrag:// link in a chat / forum
///     from instantly closing your work and yeeting you into a server
///   - gives the user a moment to read which IP they're about to join
///   - matches user mental model: launcher orchestrates, doesn't decide
///
/// Cold start: the URL is stashed in AppState too so the frontend can
/// pick it up via `get_pending_deep_link` on mount (the event emitted
/// below fires before the webview is ready).
#[cfg(desktop)]
fn handle_deep_link_url(app: &tauri::AppHandle, url: &str) {
    use tauri::{Emitter, Manager};

    match protocol::parse_url(url) {
        Ok(addr) => {
            // Auto-connect path: user opted into "skip confirmation" in
            // Settings AND has an engine configured. We launch directly
            // and stay in the tray, no window pop, no banner. If the
            // launch fails (missing/invalid engine path) we fall back
            // to the normal banner so the user can see what's wrong
            // instead of silently doing nothing.
            let cfg = config::Config::load().ok();
            let auto_connect_ok = cfg
                .as_ref()
                .map(|c| c.deep_link_auto_connect && c.engine_path.is_some())
                .unwrap_or(false);
            if auto_connect_ok {
                let engine = cfg.as_ref().and_then(|c| c.engine_path.as_deref());
                match protocol::launch(engine, addr) {
                    Ok(()) => {
                        // Log the connection to history with whatever
                        // we have (just IP:port at this point -
                        // enrichment requires the frontend's server
                        // lookup which auto-connect skips by design).
                        let state: tauri::State<AppState> = app.state();
                        state.history.log(
                            addr.ip().to_string(),
                            addr.port(),
                            None,
                            None,
                            None,
                            "auto",
                        );
                        let _ = app.emit(
                            "deep-link://result",
                            serde_json::json!({
                                "ok": true,
                                "address": addr.to_string(),
                                "url": url,
                                "auto_connect": true,
                            }),
                        );
                        return;
                    }
                    Err(_) => {
                        // Fall through to the pending banner so the
                        // user can see the failure rather than nothing
                        // happening at all.
                    }
                }
            }

            let state: tauri::State<AppState> = app.state();
            *state.pending_deep_link.lock().unwrap() = Some(url.to_string());
            let _ = app.emit(
                "deep-link://pending",
                serde_json::json!({ "address": addr.to_string(), "url": url }),
            );
            // User needs to see the window so they can press Connect.
            show_main_window(app);
        }
        Err(e) => {
            let _ = app.emit(
                "deep-link://result",
                serde_json::json!({ "ok": false, "error": e.to_string(), "url": url }),
            );
            // Surface the window so the error toast is visible.
            show_main_window(app);
        }
    }
}

