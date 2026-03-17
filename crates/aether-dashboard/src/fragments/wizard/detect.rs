use std::collections::HashMap;
use std::path::PathBuf;

use axum::extract::Query;
use axum::response::Html;
use maud::html;

/// HTMX fragment: scan workspace and show file counts by language.
/// Called when the user changes the workspace path input.
pub(crate) async fn detect_fragment(Query(params): Query<HashMap<String, String>>) -> Html<String> {
    let workspace_path = params.get("workspace_path").cloned().unwrap_or_default();

    if workspace_path.is_empty() {
        return Html(String::new());
    }

    let workspace = PathBuf::from(&workspace_path);

    if !workspace.is_dir() {
        return Html(
            html! {
                div class="text-sm text-accent-red py-2" {
                    "Directory not found: " (workspace_path)
                }
            }
            .into_string(),
        );
    }

    // Scan files in a blocking task to avoid blocking the server
    let (languages, total) = tokio::task::spawn_blocking(move || scan_workspace_quick(&workspace))
        .await
        .unwrap_or_default();

    let has_git = PathBuf::from(&workspace_path).join(".git").is_dir();

    Html(
        html! {
            div class="bg-surface-2/50 dark:bg-slate-700/40 rounded-lg p-3 space-y-2" {
                div class="flex items-center justify-between text-sm" {
                    span class="text-text-secondary dark:text-slate-300 font-medium" {
                        "Detected " (total) " files"
                    }
                    @if has_git {
                        span class="text-xs text-accent-green" { "\u{2713} Git repository" }
                    } @else {
                        span class="text-xs text-text-muted dark:text-slate-400" { "\u{2717} No Git repository" }
                    }
                }

                @if !languages.is_empty() {
                    div class="flex flex-wrap gap-2" {
                        @for (lang, count) in &languages {
                            span class="inline-flex items-center gap-1 px-2 py-0.5 text-xs rounded-full bg-accent-cyan/10 text-accent-cyan border border-accent-cyan/20" {
                                (lang)
                                span class="text-text-muted dark:text-slate-400" { "(" (count) ")" }
                            }
                        }
                    }
                } @else {
                    p class="text-xs text-text-muted dark:text-slate-400" {
                        "No recognized source files found."
                    }
                }
            }
        }
        .into_string(),
    )
}

/// Quick file scan — counts files by language. Lighter than the full
/// `detect_environment` Tauri command (no Ollama/sysinfo checks).
fn scan_workspace_quick(workspace: &std::path::Path) -> (Vec<(String, usize)>, usize) {
    let ext_map: &[(&[&str], &str)] = &[
        (&[".rs"], "Rust"),
        (&[".ts", ".tsx"], "TypeScript"),
        (&[".js", ".jsx"], "JavaScript"),
        (&[".py"], "Python"),
        (&[".go"], "Go"),
        (&[".java"], "Java"),
        (&[".c", ".h"], "C"),
        (&[".cpp", ".hpp", ".cc"], "C++"),
        (&[".cs"], "C#"),
        (&[".rb"], "Ruby"),
        (&[".swift"], "Swift"),
        (&[".kt", ".kts"], "Kotlin"),
        (&[".pdf"], "PDF"),
        (&[".docx", ".doc"], "Word"),
    ];

    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut total = 0usize;

    let walker = walkdir::WalkDir::new(workspace)
        .follow_links(false)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
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

        let file_name = entry
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        for &(exts, lang) in ext_map {
            if exts.iter().any(|ext| file_name.ends_with(ext)) {
                *counts.entry(lang).or_default() += 1;
                break;
            }
        }
    }

    let mut result: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(lang, count)| (lang.to_owned(), count))
        .collect();
    result.sort_by(|a, b| b.1.cmp(&a.1));

    (result, total)
}
