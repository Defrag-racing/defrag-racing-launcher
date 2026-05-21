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
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Logging is environment-controlled (RUST_LOG=debug) so a shipped
    // binary stays quiet unless the user flips it on while debugging.
    let _ = env_logger::try_init();

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
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }));

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(AppState::default())
        .setup(|app| {
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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

    // Emit to all webviews — Dashboard listens and renders a toast.
    // We don't propagate errors out of here because there's nobody
    // upstream to catch them; the event payload IS the error report.
    let payload = match &result {
        Ok(addr) => serde_json::json!({ "ok": true, "address": addr }),
        Err(msg) => serde_json::json!({ "ok": false, "error": msg, "url": url }),
    };
    let _ = app.emit("deep-link://result", payload);

    // Bring the launcher to the foreground so the toast is actually
    // visible — without this, the engine grabs focus and the toast
    // disappears behind it.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
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
