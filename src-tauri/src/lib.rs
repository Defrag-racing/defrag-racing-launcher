mod api;
mod cache;
mod commands;
mod comps;
mod config;
mod demo_meta;
mod demo_player;
mod engine;
mod engine_video;
mod file_assoc;
mod folders;
mod hashing;
mod history;
mod offline_maps;
mod protocol;
mod session_tracker;
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
    // Linux: pin the GDK backend to X11 BEFORE GTK initializes. The embedded
    // demo player reparents the engine's X11 window into a stage window inside
    // our own window, which only works if the launcher is itself an X11 client.
    // On a Wayland session GTK would otherwise pick the Wayland backend; forcing
    // x11 routes us through XWayland so the embed works. No-op on a real X11
    // session, and harmless on machines without Wayland.
    #[cfg(target_os = "linux")]
    std::env::set_var("GDK_BACKEND", "x11");

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

        // "Play in Defrag Launcher" on a demo while the launcher is already
        // running: the OS starts a second process, this plugin hands us its
        // argv and kills it. The file path is the whole message.
        if let Some(demo) = file_assoc::demo_path_in_args(&argv) {
            handle_open_demo(app, &demo);
        }

        // Surface the existing window so the user sees the toast emitted
        // by the deep-link handler.
        show_main_window(app);
    }));
    log_startup("single_instance plugin registered");

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Autostart is on by default - onboarding's finish step calls
        // setAutostartEnabled(true) so the watcher + defrag:// handler
        // work without the user discovering the Settings toggle. The
        // toggle stays the single source of truth and turns it off for
        // anyone who opts out. We pass HIDDEN_FLAG so the autostart-spawned
        // launcher starts in the tray instead of stealing focus on login.
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
        // Remembers the main window's size/position across runs (restores on
        // launch, saves on exit). We also save explicitly on hide-to-tray
        // below, since closing the window only hides it (prevent_close) and
        // the on-exit save wouldn't capture a resize done after the last hide.
        .plugin(tauri_plugin_window_state::Builder::default().build())
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

                // Cold start: when the launcher wasn't running and the OS
                // spawned it *because of* a defrag:// click, on_open_url
                // does not fire for that launch URL on Windows/Linux - the
                // URL only lives in our argv, exposed here via
                // get_current(). Without reading it the launcher opened but
                // never offered Connect. We pull it explicitly so the
                // pending-deep-link state is set before the webview mounts
                // and App.vue's get_pending_deep_link() picks it up. The
                // dedup guard in handle_deep_link_url stops this from
                // double-firing with on_open_url on platforms that deliver
                // the cold-start URL through both.
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    let h = app.handle().clone();
                    for url in urls {
                        log_startup(&format!("deep-link: cold-start url {}", url.as_str()));
                        handle_deep_link_url(&h, url.as_str());
                    }
                }
            }

            // The right-click entry for .dm_68, re-asserted on every start.
            // It costs one registry write, it repairs itself after the app is
            // moved or reinstalled, and it changes no default - see file_assoc.
            file_assoc::register_quietly();

            // Cold start from the file manager: the demo is in our own argv.
            // Stashed rather than emitted, because the webview does not exist
            // yet; the frontend takes it on mount.
            if let Some(demo) = file_assoc::demo_path_in_args(std::env::args().collect::<Vec<_>>())
            {
                log_startup(&format!("open-demo: cold start {}", demo.display()));
                let state: tauri::State<AppState> = app.state();
                *state.pending_open_demo.lock().unwrap() =
                    Some(demo.to_string_lossy().to_string());
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

            // Keep the comps round current in the background. The guard needs
            // it whether or not the user ever opens the Comps tab - a round
            // that starts while the launcher is running is exactly when a run
            // would otherwise be published mid-round.
            commands::spawn_comps_refresh(app.handle().clone());

            log_startup("setup() complete");
            Ok(())
        })
        .on_window_event(|window, event| {
            // Intercept the OS "close" button: hide instead of destroy
            // so the launcher stays alive in the tray. The user can
            // still quit explicitly via the tray menu's Quit item.
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Kill any embedded demo before the window vanishes into the
                // tray - otherwise the spawned engine keeps playing with no
                // transport UI to stop it (the controls live in the now-hidden
                // window).
                demo_player::stop_active_session(window.app_handle());
                // Persist the current size/position before hiding, so the next
                // show (or a later real quit) restores what the user set.
                use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                let _ = window.app_handle().save_window_state(StateFlags::all());
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
            commands::log_to_file,
            commands::save_token,
            commands::validate_token,
            commands::has_token,
            commands::clear_token,
            commands::reset_launcher,
            commands::detect_engines,
            commands::guess_demos_path,
            commands::validate_demos_path,
            commands::start_auto_upload,
            commands::stop_auto_upload,
            commands::pause_auto_upload,
            commands::resume_auto_upload,
            commands::is_auto_upload_running,
            commands::is_auto_upload_paused,
            commands::get_upload_state,
            commands::clear_upload_cache,
            commands::health_check,
            commands::health_repair,
            commands::get_cpu_throttle_pct,
            commands::set_cpu_throttle_pct_runtime,
            commands::get_rate_limit_resume_at_ms,
            commands::handle_protocol_url,
            commands::launch_engine,
            commands::launch_engine_args,
            commands::run_map_offline,
            commands::ensure_demo_map,
            commands::list_offline_maps,
            commands::offline_map_thumb,
            commands::engine_demo_resolution,
            demo_player::demo_player_start,
            demo_player::demo_player_compare_start,
            demo_player::demo_player_command,
            demo_player::demo_player_seek_relative,
            demo_player::demo_player_set_offset,
            demo_player::demo_player_seek_pane,
            demo_player::demo_player_pane_command,
            demo_player::demo_player_set_region,
            demo_player::demo_player_reposition,
            demo_player::demo_player_stop,
            commands::open_url,
            commands::get_servers,
            commands::take_pending_open_demo,
            commands::stage_demo,
            commands::demo_assoc_status,
            commands::demo_assoc_make_default,
            commands::get_pending_deep_link,
            commands::confirm_pending_deep_link,
            commands::cancel_pending_deep_link,
            commands::get_connection_history,
            commands::clear_connection_history,
            commands::get_records,
            commands::get_maps,
            commands::get_me,
            commands::list_demos,
            commands::list_demo_folders,
            commands::set_demo_folder,
            commands::get_notifications,
            commands::get_notifications_unread_count,
            commands::request_render,
            commands::get_render_status,
            commands::rendered_index,
            commands::retry_upload,
            commands::delete_demo,
            commands::notification_record_toggle,
            commands::notification_records_mark_read,
            commands::notification_records_mark_unread,
            commands::notification_system_toggle,
            commands::notification_system_mark_read,
            commands::notification_system_mark_unread,
            commands::set_autostart_enabled,
            commands::is_autostart_enabled,
            commands::get_comps,
            commands::refresh_comps,
            commands::comps_enter,
            commands::comps_upload_normally,
            commands::comps_mark_intro_seen,
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
        state.comps.clone(),
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

/// A demo file arrived from the file manager while we were already running.
///
/// Unlike a defrag:// URL, this needs no confirmation step. A URL can be
/// clicked by accident in a chat window and would drop somebody into a server;
/// this is a file the person deliberately right-clicked, and the only sensible
/// answer to "play this demo" is to play it.
#[cfg(desktop)]
fn handle_open_demo(app: &tauri::AppHandle, demo: &std::path::Path) {
    use tauri::{Emitter, Manager};

    let path = demo.to_string_lossy().to_string();
    log_startup(&format!("open-demo: {}", path));

    // Stashed as well as emitted: if the window is still starting up, nothing
    // is listening yet, and the frontend takes it on mount instead.
    {
        let state: tauri::State<AppState> = app.state();
        *state.pending_open_demo.lock().unwrap() = Some(path.clone());
    }

    let _ = app.emit("open-demo", path);
    show_main_window(app);
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

    // Drop a duplicate delivery of the same URL. On a cold start the OS
    // hands us the launch URL through both our explicit `get_current()`
    // read in setup() and (on some platforms) the plugin's `on_open_url`
    // callback; without this guard the auto-connect path would launch the
    // engine twice for one click. 1.5s is comfortably longer than the gap
    // between the two deliveries and shorter than any intentional reclick.
    {
        let state: tauri::State<AppState> = app.state();
        let mut last = state.last_deep_link.lock().unwrap();
        if let Some((prev_url, at)) = last.as_ref() {
            if prev_url == url && at.elapsed() < std::time::Duration::from_millis(1500) {
                log_startup(&format!("deep-link: dropping duplicate {}", url));
                return;
            }
        }
        *last = Some((url.to_string(), std::time::Instant::now()));
    }

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
                match protocol::launch(engine, &addr) {
                    Ok(child) => {
                        // Log the connection to history with whatever
                        // we have (just IP:port at this point -
                        // enrichment requires the frontend's server
                        // lookup which auto-connect skips by design),
                        // then track the engine process so the server's
                        // map rotations get logged onto this entry until
                        // the game closes.
                        let state: tauri::State<AppState> = app.state();
                        let session_id = state.history.log(
                            addr.host().to_string(),
                            addr.port(),
                            None,
                            None,
                            None,
                            "auto",
                        );
                        state.session_tracker.register(
                            child,
                            addr.host().to_string(),
                            addr.port(),
                            session_id,
                            None,
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

