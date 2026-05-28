//! Tauri command surface — the thin RPC layer the webview calls via `invoke`.
//! Each command borrows the shared [`Core`] and delegates; all real logic lives
//! in `core.rs`. Errors are returned as strings so they surface cleanly in JS.

use crate::core::{Core, OpenProjectResult, ProjectFile};
use crate::model::*;
use std::sync::Arc;
use tauri::State;

type R<T> = Result<T, String>;

// ---- sessions --------------------------------------------------------------

#[tauri::command]
pub fn create_session(core: State<Arc<Core>>, spec: SessionSpec) -> R<SessionId> {
    match spec.mode {
        SessionMode::Headless => core.spawn_headless(spec),
        SessionMode::Interactive => core.spawn_interactive(spec),
        SessionMode::Shell => core.spawn_shell(spec),
    }
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_sessions(core: State<Arc<Core>>) -> Vec<SessionInfo> {
    core.list_sessions()
}

#[tauri::command]
pub fn attach_session(core: State<Arc<Core>>, id: String) -> R<()> {
    core.attach(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn detach_session(core: State<Arc<Core>>, id: String) {
    core.detach(&id);
}

#[tauri::command]
pub fn send_input(core: State<Arc<Core>>, id: String, data: String) -> R<()> {
    core.send_input(&id, &data).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn send_message(core: State<Arc<Core>>, id: String, prompt: String) -> R<()> {
    core.send_message(&id, &prompt).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resize_terminal(core: State<Arc<Core>>, id: String, cols: u16, rows: u16) -> R<()> {
    core.resize(&id, cols, rows).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn broadcast(core: State<Arc<Core>>, text: String) -> usize {
    core.broadcast(&text)
}

#[tauri::command]
pub fn kill_session(core: State<Arc<Core>>, id: String) -> R<()> {
    core.kill(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_session(core: State<Arc<Core>>, id: String) -> R<()> {
    core.remove(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_logs(core: State<Arc<Core>>, id: String, limit: Option<i64>) -> Vec<LogEntry> {
    core.get_logs(&id, limit.unwrap_or(500))
}

// ---- tasks -----------------------------------------------------------------

#[tauri::command]
pub fn enqueue_task(core: State<Arc<Core>>, task: Task) -> TaskId {
    core.enqueue_task(task)
}

#[tauri::command]
pub fn list_tasks(core: State<Arc<Core>>) -> Vec<Task> {
    core.list_tasks()
}

#[tauri::command]
pub fn cancel_task(core: State<Arc<Core>>, id: String) {
    core.cancel_task(&id);
}

// ---- metrics + config ------------------------------------------------------

#[tauri::command]
pub fn get_metrics(core: State<Arc<Core>>) -> SystemMetrics {
    *core.metrics.lock()
}

#[tauri::command]
pub fn get_config(core: State<Arc<Core>>) -> OrchestratorConfig {
    core.get_config()
}

#[tauri::command]
pub fn set_config(core: State<Arc<Core>>, config: OrchestratorConfig) {
    core.set_config(config);
}

// ---- snapshots -------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct SnapshotMeta {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

#[tauri::command]
pub fn save_snapshot(core: State<Arc<Core>>, name: String) -> R<String> {
    core.save_snapshot(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_snapshots(core: State<Arc<Core>>) -> Vec<SnapshotMeta> {
    core.list_snapshots()
        .into_iter()
        .map(|(id, name, created_at)| SnapshotMeta { id, name, created_at })
        .collect()
}

#[tauri::command]
pub fn restore_snapshot(core: State<Arc<Core>>, id: String) -> R<usize> {
    core.restore_snapshot(&id).map_err(|e| e.to_string())
}

// ---- project files ---------------------------------------------------------

#[tauri::command]
pub fn open_project(core: State<Arc<Core>>, path: String) -> R<OpenProjectResult> {
    core.open_project(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_project(core: State<Arc<Core>>, path: String) -> R<()> {
    core.save_project(&path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn write_project(core: State<Arc<Core>>, path: String, project: ProjectFile) -> R<()> {
    core.write_project(&path, &project).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_session(core: State<Arc<Core>>, id: String, name: String) -> R<()> {
    core.rename(&id, &name).map_err(|e| e.to_string())
}
