mod api;
mod commands;
mod config;
mod engine;
mod hashing;
mod token;
mod watcher;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Logging is environment-controlled (RUST_LOG=debug) so a shipped
    // binary stays quiet unless the user flips it on while debugging.
    let _ = env_logger::try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(AppState::default())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
