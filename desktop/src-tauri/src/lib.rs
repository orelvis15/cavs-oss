//! CAVS Desktop — Tauri backend.
//!
//! Thin, structured command layer over the shared CAVS Rust core
//! (`cavs-sdk-core`). No CAVS logic is duplicated here: operations are
//! dispatched to the same engine the CLI and SDKs use.

mod appstate;
mod commands;
mod db;
mod error;
mod server;
mod storage;

use appstate::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            use tauri::Manager;
            let state = AppState::new()
                .map_err(|e| format!("[{}] {}", e.code, e.description))?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::save_settings,
            commands::list_projects,
            commands::create_project,
            commands::update_project,
            commands::delete_project,
            commands::list_operations,
            commands::list_project_operations,
            commands::get_operation,
            commands::delete_operation,
            commands::run_operation,
            commands::open_path,
            commands::detect_tools,
            commands::server_start,
            commands::server_stop,
            commands::server_status,
            commands::server_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CAVS Desktop");
}
