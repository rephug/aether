use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::support::{self, DashboardState};

// ── Preset structs (mirror of aetherd::context_presets) ────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresetConfig {
    pub preset: PresetMeta,
    pub context: PresetContextSettings,
    #[serde(default)]
    pub task_template: Option<PresetTaskTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresetMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresetContextSettings {
    #[serde(default = "default_budget")]
    pub budget: usize,
    #[serde(default = "default_depth")]
    pub depth: u32,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_context_lines")]
    pub context_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PresetTaskTemplate {
    pub template: String,
}

fn default_budget() -> usize {
    32_000
}
fn default_depth() -> u32 {
    2
}
fn default_format() -> String {
    "markdown".to_owned()
}
fn default_context_lines() -> usize {
    3
}

// ── API data types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PresetEntry {
    pub name: String,
    pub description: String,
    pub budget: usize,
    pub depth: u32,
    pub format: String,
    pub include: Vec<String>,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PresetsData {
    pub presets: Vec<PresetEntry>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreatePresetRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_budget")]
    pub budget: usize,
    #[serde(default = "default_depth")]
    pub depth: u32,
}

// ── Built-in presets (matches aetherd::context_presets::builtin_presets) ──

fn builtin_preset_entries() -> Vec<PresetEntry> {
    vec![
        PresetEntry {
            name: "quick".to_owned(),
            description: "Quick question about a symbol".to_owned(),
            budget: 8_000,
            depth: 1,
            format: "markdown".to_owned(),
            include: vec!["sir".to_owned(), "source".to_owned()],
            is_builtin: true,
        },
        PresetEntry {
            name: "review".to_owned(),
            description: "Code review context".to_owned(),
            budget: 32_000,
            depth: 2,
            format: "markdown".to_owned(),
            include: vec![
                "sir".to_owned(),
                "source".to_owned(),
                "graph".to_owned(),
                "coupling".to_owned(),
                "health".to_owned(),
                "tests".to_owned(),
            ],
            is_builtin: true,
        },
        PresetEntry {
            name: "deep".to_owned(),
            description: "Deep analysis or refactor planning".to_owned(),
            budget: 64_000,
            depth: 3,
            format: "markdown".to_owned(),
            include: vec![
                "sir".to_owned(),
                "source".to_owned(),
                "graph".to_owned(),
                "coupling".to_owned(),
                "health".to_owned(),
                "drift".to_owned(),
                "memory".to_owned(),
                "tests".to_owned(),
            ],
            is_builtin: true,
        },
        PresetEntry {
            name: "overview".to_owned(),
            description: "Project-level health check".to_owned(),
            budget: 16_000,
            depth: 0,
            format: "markdown".to_owned(),
            include: vec!["sir".to_owned(), "health".to_owned(), "drift".to_owned()],
            is_builtin: true,
        },
    ]
}

// ── Handlers ───────────────────────────────────────────────────────────

pub(crate) async fn presets_handler(State(state): State<Arc<DashboardState>>) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_presets_data(&shared.workspace)).await {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => support::json_internal_error(err),
    }
}

pub(crate) async fn create_preset_handler(
    State(state): State<Arc<DashboardState>>,
    Json(body): Json<CreatePresetRequest>,
) -> impl IntoResponse {
    let workspace = state.shared.workspace.clone();
    match create_preset(&workspace, &body) {
        Ok(()) => {
            let data = load_presets_data(&workspace).unwrap_or(PresetsData {
                presets: Vec::new(),
                total: 0,
            });
            support::api_json(state.shared.as_ref(), data).into_response()
        }
        Err(err) => support::json_internal_error(err),
    }
}

pub(crate) async fn delete_preset_handler(
    State(state): State<Arc<DashboardState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let workspace = state.shared.workspace.clone();
    match delete_preset(&workspace, &name) {
        Ok(()) => {
            let data = load_presets_data(&workspace).unwrap_or(PresetsData {
                presets: Vec::new(),
                total: 0,
            });
            support::api_json(state.shared.as_ref(), data).into_response()
        }
        Err(err) => support::json_internal_error(err),
    }
}

// ── Data loading ───────────────────────────────────────────────────────

pub(crate) fn load_presets_data(workspace: &std::path::Path) -> Result<PresetsData, String> {
    let mut by_name: BTreeMap<String, PresetEntry> = builtin_preset_entries()
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();

    let presets_dir = workspace.join(".aether/presets");
    if presets_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&presets_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(&path)
                && let Ok(parsed) = toml::from_str::<PresetConfig>(&raw)
            {
                by_name.insert(
                    parsed.preset.name.clone(),
                    PresetEntry {
                        name: parsed.preset.name,
                        description: parsed.preset.description,
                        budget: parsed.context.budget,
                        depth: parsed.context.depth,
                        format: parsed.context.format,
                        include: parsed.context.include,
                        is_builtin: false,
                    },
                );
            }
        }
    }

    let presets: Vec<PresetEntry> = by_name.into_values().collect();
    let total = presets.len();
    Ok(PresetsData { presets, total })
}

fn create_preset(workspace: &std::path::Path, body: &CreatePresetRequest) -> Result<(), String> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err("preset name cannot be empty".to_owned());
    }
    if builtin_preset_entries().iter().any(|p| p.name == name) {
        return Err(format!("cannot overwrite built-in preset '{name}'"));
    }

    let safe_name: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let presets_dir = workspace.join(".aether/presets");
    std::fs::create_dir_all(&presets_dir)
        .map_err(|e| format!("failed to create presets dir: {e}"))?;

    let config = PresetConfig {
        preset: PresetMeta {
            name: name.to_owned(),
            description: body.description.clone(),
        },
        context: PresetContextSettings {
            budget: body.budget,
            depth: body.depth,
            include: Vec::new(),
            exclude: Vec::new(),
            format: default_format(),
            context_lines: default_context_lines(),
        },
        task_template: None,
    };
    let toml_str =
        toml::to_string_pretty(&config).map_err(|e| format!("failed to serialize preset: {e}"))?;
    let path = presets_dir.join(format!("{safe_name}.toml"));
    std::fs::write(&path, toml_str).map_err(|e| format!("failed to write preset: {e}"))?;
    Ok(())
}

fn delete_preset(workspace: &std::path::Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if builtin_preset_entries().iter().any(|p| p.name == name) {
        return Err(format!("cannot delete built-in preset '{name}'"));
    }

    let safe_name: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = workspace
        .join(".aether/presets")
        .join(format!("{safe_name}.toml"));
    if !path.exists() {
        return Err(format!("preset '{name}' not found"));
    }
    std::fs::remove_file(&path).map_err(|e| format!("failed to delete preset: {e}"))?;
    Ok(())
}
