//! Application entry point: opens the database, builds the orchestrator
//! [`Core`], starts the background scheduler + monitor loops, and registers the
//! command surface with Tauri.

mod commands;
mod core;
mod events;
mod launcher;
mod model;
mod monitor;
mod protocol;
mod ring;
mod scheduler;
mod session;
mod storage;

use crate::core::Core;
use std::thread;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Database lives under the platform app-data directory.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("multi-ai-harness"));
            let db_path = data_dir.join("harness.sqlite");
            let conn = storage::open(&db_path).expect("failed to open database");

            let core = Core::new(app.handle().clone(), conn);

            // Background loops own clones of the Arc.
            {
                let core = core.clone();
                thread::Builder::new()
                    .name("scheduler".into())
                    .spawn(move || scheduler::run(core))
                    .ok();
            }
            {
                let core = core.clone();
                thread::Builder::new()
                    .name("monitor".into())
                    .spawn(move || monitor::run(core))
                    .ok();
            }

            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::list_sessions,
            commands::attach_session,
            commands::detach_session,
            commands::send_input,
            commands::send_message,
            commands::resize_terminal,
            commands::broadcast,
            commands::kill_session,
            commands::remove_session,
            commands::get_logs,
            commands::enqueue_task,
            commands::list_tasks,
            commands::cancel_task,
            commands::get_metrics,
            commands::get_config,
            commands::set_config,
            commands::save_snapshot,
            commands::list_snapshots,
            commands::restore_snapshot,
            commands::open_project,
            commands::save_project,
            commands::write_project,
            commands::rename_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
