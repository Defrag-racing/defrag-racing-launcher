mod api;
mod cache;
mod commands;
mod config;
mod engine;
mod hashing;
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
/// isn't ambushed by a popup every boot — they explicitly clicked
/// "Show" from the tray to see the dashboard.
const HIDDEN_FLAG: &str = "--hidden";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Logging is environment-controlled (RUST_LOG=debug) so a shipped
    // binary stays quiet unless the user flips it on while debugging.
    let _ = env_logger::try_init();

    let started_hidden = std::env::args().any(|a| a == HIDDEN_FLAG);

    let builder = tauri::Builder::default();

    // single-instance MUST be the first plugin registered — when a second
    // launcher process is spawned (e.g. by a defrag:// click while the
    // launcher is already open) this plugin hands the new process's argv
    // to the existing one and exits the new one. Anything later would
    // run in both processes briefly. Shadow `builder` rather than mutate
    // so macOS (where the cfg block is dead code) doesn't trip an
    // unused_mut warning.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
        // argv from the second instance — on Windows + Linux a
        // defrag://… deep link arrives as one of the argv entries.
        // Hand it off to the same handler the deep-link plugin uses
        // so both paths produce identical behavior.
        handle_deep_link_argv(app, &argv);
        // Surface the existing window so the user sees the toast.
        show_main_window(app);
    }));

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
            // Hide on launch if we were started by the OS at login —
            // user explicitly opted into autostart and expects a quiet
            // background process, not a pop-up.
            if started_hidden {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            // Tray icon — keeps the launcher alive after the user
            // closes the window so the demo watcher + defrag:// handler
            // keep doing their job. Without this the process would exit
            // on last-window-close (default Tauri behavior).
            build_tray(app)?;

            // Register defrag:// scheme at runtime — needed in dev where
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
            commands::is_auto_upload_running,
            commands::get_upload_state,
            commands::clear_upload_cache,
            commands::handle_protocol_url,
            commands::set_autostart_enabled,
            commands::is_autostart_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Build the tray icon, attach a Show / Quit menu, and wire left-click
/// to "Show" so a single click on the tray icon brings the dashboard
/// up without forcing the user to discover the right-click menu first.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    // Prefer the bundle's window icon — Windows packs that into the .exe
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

/// Bring the main window from hidden / minimized into the foreground.
/// Used by tray clicks, single-instance forwarding, and the deep-link
/// handler — anywhere we want the user to actually see the UI.
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Single source of truth for what happens when a defrag:// URL arrives
/// — from any path (deep-link plugin event, single-instance argv
/// forwarding, or manual invocation in tests). Validates, launches the
/// engine, and emits a `deep-link://result` event so the frontend can
/// show a toast about success or failure.
#[cfg(desktop)]
fn handle_deep_link_url(app: &tauri::AppHandle, url: &str) {
    use tauri::Emitter;

    let result = (|| -> Result<String, String> {
        let addr = protocol::parse_url(url).map_err(|e| e.to_string())?;
        let cfg = config::Config::load().map_err(|e| e.to_string())?;
        protocol::launch(cfg.engine_path.as_deref(), addr).map_err(|e| e.to_string())?;
        Ok(addr.to_string())
    })();

    let payload = match &result {
        Ok(addr) => serde_json::json!({ "ok": true, "address": addr }),
        Err(msg) => serde_json::json!({ "ok": false, "error": msg, "url": url }),
    };
    let _ = app.emit("deep-link://result", payload);

    // Bring the launcher to the foreground so the toast is actually
    // visible — without this, the engine grabs focus and the toast
    // disappears behind it.
    show_main_window(app);
}

/// Sift defrag:// URLs out of a process argv vector. Used by the
/// single-instance plugin which receives the entire argv of the second
/// process (program name + flags + the URL). We don't trust positional
/// arguments — we look for the first arg that starts with `defrag://`.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn handle_deep_link_argv(app: &tauri::AppHandle, argv: &[String]) {
    for arg in argv {
        if arg.starts_with("defrag://") {
            handle_deep_link_url(app, arg);
            return;
        }
    }
}
