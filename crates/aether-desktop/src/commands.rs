use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use crate::updater::UpdateInfo;
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

// --- Update commands ---

#[tauri::command]
pub async fn check_for_update(app_handle: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    crate::updater::check(&app_handle).await
}

#[tauri::command]
pub async fn install_update(app_handle: tauri::AppHandle) -> Result<String, String> {
    crate::updater::install(&app_handle).await?;
    Ok("Update installed".to_owned())
}

// --- Update preferences ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePreferences {
    pub auto_check: bool,
    pub frequency: String,
    pub include_prerelease: bool,
}

impl Default for UpdatePreferences {
    fn default() -> Self {
        Self {
            auto_check: true,
            frequency: "launch".to_owned(),
            include_prerelease: false,
        }
    }
}

#[tauri::command]
pub fn get_update_preferences(app_handle: tauri::AppHandle) -> UpdatePreferences {
    let path = match app_handle.path().app_data_dir() {
        Ok(p) => p.join("update_preferences.json"),
        Err(_) => return UpdatePreferences::default(),
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => UpdatePreferences::default(),
    }
}

#[tauri::command]
pub fn set_update_preferences(
    app_handle: tauri::AppHandle,
    prefs: UpdatePreferences,
) -> Result<String, String> {
    let dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("update_preferences.json");
    let json = serde_json::to_string_pretty(&prefs).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok("Preferences saved".to_owned())
}
