use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// Information about an available update, returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub body: Option<String>,
    pub date: Option<String>,
}

/// Check if an update is available. Returns `None` if already up to date.
pub async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;

    match update {
        Some(u) => Ok(Some(UpdateInfo {
            version: u.version.clone(),
            body: u.body.clone(),
            date: u.date.map(|d| d.to_string()),
        })),
        None => Ok(None),
    }
}

/// Download and install a pending update, then restart the app.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;

    let Some(u) = update else {
        return Err("No update available".to_owned());
    };

    u.download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}
