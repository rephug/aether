use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReport {
    pub source_files: Vec<LanguageCount>,
    pub total_files: usize,
    pub has_git: bool,
    pub ollama_running: bool,
    pub ollama_models: Vec<String>,
    pub has_pdftotext: bool,
    pub has_gemini_key: bool,
    pub gemini_key_env: String,
    pub available_ram_mb: u64,
    pub available_disk_mb: u64,
    pub workspace_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LanguageCount {
    pub language: String,
    pub count: usize,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEstimate {
    pub total_files: usize,
    pub symbols_approx: usize,
    pub estimated_seconds: u64,
    pub provider_label: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardConfig {
    pub workspace_path: String,
    pub provider: String,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
    pub enable_batch: bool,
    pub enable_continuous: bool,
}

// ---------------------------------------------------------------------------
// Last-workspace persistence (mirrors update_preferences pattern)
// ---------------------------------------------------------------------------

pub fn load_last_workspace_path(app: &tauri::AppHandle) -> Option<String> {
    let dir = app.path().app_data_dir().ok()?;
    let path = dir.join("last_workspace.json");
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&contents)
        .ok()?
        .get("workspace")?
        .as_str()
        .map(ToOwned::to_owned)
}

fn save_last_workspace_to_disk(app: &tauri::AppHandle, workspace: &str) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("last_workspace.json");
    let json = serde_json::json!({ "workspace": workspace });
    let content = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(path, content).map_err(|e| e.to_string())?;
    Ok(())
}

/// Pre-Tauri workspace resolution: reads last_workspace.json without an AppHandle.
///
/// Computes the app data dir manually using the app identifier from tauri.conf.json.
/// On Linux: `$XDG_DATA_HOME/com.aether.desktop/` or `$HOME/.local/share/com.aether.desktop/`
pub fn load_last_workspace_pre_tauri() -> Option<String> {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    let path = data_dir
        .join("com.aether.desktop")
        .join("last_workspace.json");
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&contents)
        .ok()?
        .get("workspace")?
        .as_str()
        .map(ToOwned::to_owned)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn pick_directory(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    app.dialog().file().pick_folder(move |folder_path| {
        let result = folder_path.map(|p| p.to_string());
        let _ = tx.send(result);
    });
    rx.await.ok().flatten()
}

#[tauri::command]
pub async fn detect_environment(workspace_path: String) -> Result<EnvironmentReport, String> {
    let workspace = PathBuf::from(&workspace_path);
    if !workspace.is_dir() {
        return Err(format!("Not a directory: {workspace_path}"));
    }

    // Count files by extension
    let (language_counts, total_files) = scan_workspace_files(&workspace);

    // Check for .git
    let has_git = workspace.join(".git").is_dir();

    // Check Ollama
    let (ollama_running, ollama_models) = check_ollama().await;

    // Check pdftotext
    let has_pdftotext = check_pdftotext();

    // Check GEMINI_API_KEY
    let gemini_key_env = "GEMINI_API_KEY".to_owned();
    let has_gemini_key = std::env::var(&gemini_key_env).is_ok();

    // RAM via sysinfo
    let available_ram_mb = {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.total_memory() / (1024 * 1024)
    };

    // Disk space
    let available_disk_mb = fs2_available_space(&workspace);

    // Classify workspace type
    let code_count: usize = language_counts
        .iter()
        .filter(|lc| !matches!(lc.language.as_str(), "PDF" | "Word"))
        .map(|lc| lc.count)
        .sum();
    let doc_count: usize = language_counts
        .iter()
        .filter(|lc| matches!(lc.language.as_str(), "PDF" | "Word"))
        .map(|lc| lc.count)
        .sum();
    let workspace_type = if code_count > 0 && doc_count == 0 {
        "code"
    } else if doc_count > 0 && code_count == 0 {
        "documents"
    } else {
        "hybrid"
    }
    .to_owned();

    Ok(EnvironmentReport {
        source_files: language_counts,
        total_files,
        has_git,
        ollama_running,
        ollama_models,
        has_pdftotext,
        has_gemini_key,
        gemini_key_env,
        available_ram_mb,
        available_disk_mb,
        workspace_type,
    })
}

#[tauri::command]
pub fn estimate_index_time(file_count: usize, provider: String) -> IndexEstimate {
    let symbols_approx = file_count * 15;
    let (estimated_seconds, provider_label) = match provider.as_str() {
        "mock" => (0, "Mock (instant)"),
        "gemini" | "cloud" => ((symbols_approx as f64 * 0.5) as u64, "Gemini Flash (cloud)"),
        "qwen3_local" | "local" => (
            (symbols_approx as f64 * 1.2) as u64,
            "Ollama / Local (offline)",
        ),
        _ => ((symbols_approx as f64 * 0.5) as u64, "Auto"),
    };
    IndexEstimate {
        total_files: file_count,
        symbols_approx,
        estimated_seconds,
        provider_label: provider_label.to_owned(),
    }
}

#[allow(unreachable_code)]
#[tauri::command]
pub fn finalize_wizard(app: tauri::AppHandle, config: WizardConfig) -> Result<(), String> {
    let workspace = PathBuf::from(&config.workspace_path);

    // Build AetherConfig from wizard choices
    let mut aether_config = aether_config::AetherConfig::default();

    match config.provider.as_str() {
        "gemini" | "cloud" => {
            aether_config.inference.provider = aether_config::InferenceProviderKind::Gemini;
            aether_config.embeddings.provider = aether_config::EmbeddingProviderKind::GeminiNative;
            aether_config.embeddings.enabled = true;
            if let Some(ref key_env) = config.api_key_env {
                aether_config.inference.api_key_env = key_env.clone();
                aether_config.embeddings.api_key_env = Some(key_env.clone());
            }
        }
        "qwen3_local" | "local" => {
            aether_config.inference.provider = aether_config::InferenceProviderKind::Qwen3Local;
            aether_config.embeddings.provider = aether_config::EmbeddingProviderKind::Qwen3Local;
            aether_config.embeddings.enabled = true;
            if let Some(ref endpoint) = config.endpoint {
                aether_config.inference.endpoint = Some(endpoint.clone());
            }
        }
        _ => {
            // Leave defaults (Auto provider, embeddings disabled)
        }
    }

    if let Some(ref model) = config.model {
        aether_config.inference.model = Some(model.clone());
    }

    if config.enable_batch {
        aether_config.batch = Some(aether_config::BatchConfig::default());
    }
    if config.enable_continuous {
        aether_config.continuous = Some(aether_config::ContinuousConfig::default());
    }

    // Write config.toml
    aether_config::save_workspace_config(&workspace, &aether_config)
        .map_err(|e| format!("Failed to save config: {e}"))?;

    // Persist last workspace
    save_last_workspace_to_disk(&app, &config.workspace_path)?;

    // Restart into normal mode
    app.restart();

    Ok(())
}

#[tauri::command]
pub fn get_last_workspace(app: tauri::AppHandle) -> Option<String> {
    load_last_workspace_path(&app)
}

#[tauri::command]
pub fn save_last_workspace(app: tauri::AppHandle, path: String) -> Result<(), String> {
    save_last_workspace_to_disk(&app, &path)
}

#[tauri::command]
pub fn is_first_run(workspace_path: String) -> bool {
    let config_path = aether_config::config_path(&workspace_path);
    !config_path.exists()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Walk the workspace directory and count files by language.
fn scan_workspace_files(workspace: &Path) -> (Vec<LanguageCount>, usize) {
    let ext_to_lang: &[(&[&str], &str)] = &[
        (&[".rs"], "Rust"),
        (&[".ts", ".tsx"], "TypeScript"),
        (&[".js", ".jsx"], "JavaScript"),
        (&[".py"], "Python"),
        (&[".go"], "Go"),
        (&[".java"], "Java"),
        (&[".c", ".h"], "C"),
        (&[".cpp", ".hpp", ".cc", ".cxx"], "C++"),
        (&[".cs"], "C#"),
        (&[".rb"], "Ruby"),
        (&[".swift"], "Swift"),
        (&[".kt", ".kts"], "Kotlin"),
        (&[".pdf"], "PDF"),
        (&[".docx", ".doc"], "Word"),
    ];

    let mut counts: HashMap<&str, (usize, Vec<String>)> = HashMap::new();
    let mut total = 0usize;

    let walker = walkdir::WalkDir::new(workspace)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            // Skip hidden dirs, node_modules, target, .git, .aether
            if e.file_type().is_dir() {
                return !name.starts_with('.')
                    && name != "node_modules"
                    && name != "target"
                    && name != "__pycache__"
                    && name != "venv"
                    && name != ".venv";
            }
            true
        });

    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        total += 1;

        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        for &(exts, lang) in ext_to_lang {
            if exts.iter().any(|ext| file_name.ends_with(ext)) {
                let entry = counts.entry(lang).or_insert_with(|| (0, Vec::new()));
                entry.0 += 1;
                for ext in exts {
                    let ext_owned = ext.to_string();
                    if !entry.1.contains(&ext_owned) {
                        entry.1.push(ext_owned);
                    }
                }
                break;
            }
        }
    }

    let mut result: Vec<LanguageCount> = counts
        .into_iter()
        .map(|(lang, (count, extensions))| LanguageCount {
            language: lang.to_owned(),
            count,
            extensions,
        })
        .collect();
    result.sort_by(|a, b| b.count.cmp(&a.count));

    (result, total)
}

/// Check if Ollama is running and list available models.
async fn check_ollama() -> (bool, Vec<String>) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return (false, Vec::new()),
    };

    // Check version endpoint
    let running = client
        .get("http://127.0.0.1:11434/api/version")
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    if !running {
        return (false, Vec::new());
    }

    // Get model list
    let models = match client.get("http://127.0.0.1:11434/api/tags").send().await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                            .map(ToOwned::to_owned)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        Err(_) => Vec::new(),
    };

    (true, models)
}

/// Check if pdftotext is available.
fn check_pdftotext() -> bool {
    std::process::Command::new("which")
        .arg("pdftotext")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get available disk space in MB at the given path, or 0 on failure.
fn fs2_available_space(path: &Path) -> u64 {
    // Try parent directories until we find one that exists
    let mut check_path = path.to_path_buf();
    loop {
        if check_path.exists() {
            return fs2::available_space(&check_path)
                .map(|b| b / (1024 * 1024))
                .unwrap_or(0);
        }
        if !check_path.pop() {
            return 0;
        }
    }
}
