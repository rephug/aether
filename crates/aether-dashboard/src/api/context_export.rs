use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const MAX_FILES: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PresetInfo {
    pub name: String,
    pub description: String,
    pub budget: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ContextExportData {
    pub available_files: Vec<String>,
    pub available_presets: Vec<PresetInfo>,
    pub formats: Vec<String>,
    pub default_budget: usize,
}

pub(crate) async fn context_export_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_context_export_data(shared.as_ref()))
        .await
    {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}

pub(crate) fn load_context_export_data(shared: &SharedState) -> Result<ContextExportData, String> {
    // Get distinct file paths from the symbols table
    let available_files =
        if let Ok(Some(conn)) = support::open_meta_sqlite_ro(shared.workspace.as_path()) {
            let mut stmt = conn
                .prepare("SELECT DISTINCT file_path FROM symbols ORDER BY file_path ASC LIMIT ?1")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(rusqlite::params![MAX_FILES as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect::<Vec<_>>()
        } else {
            Vec::new()
        };

    // List presets from .aether/presets/ directory + built-ins
    let available_presets = load_preset_summaries(shared);

    Ok(ContextExportData {
        available_files,
        available_presets,
        formats: vec![
            "markdown".to_owned(),
            "xml".to_owned(),
            "compact".to_owned(),
        ],
        default_budget: 32_000,
    })
}

fn load_preset_summaries(shared: &SharedState) -> Vec<PresetInfo> {
    let mut presets = vec![
        PresetInfo {
            name: "quick".to_owned(),
            description: "Quick question about a symbol".to_owned(),
            budget: 8_000,
        },
        PresetInfo {
            name: "review".to_owned(),
            description: "Code review context".to_owned(),
            budget: 32_000,
        },
        PresetInfo {
            name: "deep".to_owned(),
            description: "Deep analysis or refactor planning".to_owned(),
            budget: 64_000,
        },
        PresetInfo {
            name: "overview".to_owned(),
            description: "Project-level health check".to_owned(),
            budget: 16_000,
        },
    ];

    // Read user presets from .aether/presets/*.toml
    let presets_dir = shared.workspace.join(".aether/presets");
    if presets_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&presets_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(&path)
                && let Ok(parsed) = toml::from_str::<super::presets::PresetConfig>(&raw)
            {
                presets.push(PresetInfo {
                    name: parsed.preset.name,
                    description: parsed.preset.description,
                    budget: parsed.context.budget,
                });
            }
        }
    }

    presets
}
