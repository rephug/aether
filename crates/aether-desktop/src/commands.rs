use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::State;

use crate::{AppState, PauseFlag};

#[derive(Debug, Clone, Serialize)]
pub struct StatusResponse {
    pub workspace: String,
    pub symbol_count: usize,
    pub state: String,
    pub paused: bool,
    pub dashboard_port: u16,
}

#[tauri::command]
pub fn get_status(app_state: State<'_, AppState>, pause: State<'_, PauseFlag>) -> StatusResponse {
    let paused = pause.0.load(Ordering::Relaxed);
    StatusResponse {
        workspace: app_state.workspace.display().to_string(),
        symbol_count: app_state.symbol_count(),
        state: if paused {
            "paused".to_owned()
        } else {
            "idle".to_owned()
        },
        paused,
        dashboard_port: app_state.dashboard_port,
    }
}

#[tauri::command]
pub fn get_workspace_path(app_state: State<'_, AppState>) -> String {
    app_state.workspace.display().to_string()
}

#[tauri::command]
pub fn pause_indexing(pause: State<'_, PauseFlag>) -> Result<String, String> {
    pause.0.store(true, Ordering::Relaxed);
    Ok("Indexing paused".to_owned())
}

#[tauri::command]
pub fn resume_indexing(pause: State<'_, PauseFlag>) -> Result<String, String> {
    pause.0.store(false, Ordering::Relaxed);
    Ok("Indexing resumed".to_owned())
}

#[allow(unreachable_code)]
#[tauri::command]
pub fn restart_app(app_handle: tauri::AppHandle) -> Result<String, String> {
    app_handle.restart();
    Ok("Restarting".to_owned())
}
