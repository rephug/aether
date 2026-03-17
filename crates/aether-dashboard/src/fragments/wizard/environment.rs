use std::collections::HashMap;

use maud::{Markup, html};

use super::{back_button, next_button};

/// Step 3: Environment check results.
///
/// Renders a static container with a JS script that calls the
/// `detect_environment` Tauri command and updates the DOM with results
/// using safe DOM manipulation (no innerHTML).
pub(crate) fn render(params: &HashMap<String, String>) -> Markup {
    let workspace = params.get("workspace_path").cloned().unwrap_or_default();

    html! {
        div class="space-y-5" {
            h2 class="text-lg font-semibold text-text-primary dark:text-slate-100" {
                "Environment Check"
            }

            p class="text-sm text-text-secondary dark:text-slate-300" {
                "Checking your system for AETHER\u{2019}s dependencies\u{2026}"
            }

            // Hidden field to carry workspace_path forward
            input type="hidden" name="workspace_path" value=(workspace) {}

            // Results container — filled by JavaScript
            div id="env-check-results" {
                div class="space-y-2 animate-pulse" {
                    @for _ in 0..6 {
                        div class="h-8 bg-surface-2/50 dark:bg-slate-700/40 rounded" {}
                    }
                }
            }

            // Navigation
            div class="flex justify-between pt-4" {
                (back_button(2, params))
                (next_button(4))
            }
        }

        // Script to call detect_environment Tauri command
        script {
            (maud::PreEscaped(format!(r#"
(function() {{
    var ws = {workspace_json};
    var container = document.getElementById('env-check-results');
    if (!ws || !window.__TAURI__) {{
        container.textContent = '';
        var p = document.createElement('p');
        p.className = 'text-sm text-accent-orange';
        p.textContent = 'Environment detection requires AETHER Desktop with a workspace selected.';
        container.appendChild(p);
        return;
    }}
    window.__TAURI__.core.invoke('detect_environment', {{ workspacePath: ws }})
        .then(function(report) {{
            container.textContent = '';
            var list = document.createElement('div');
            list.className = 'space-y-2';

            // Source files
            if (report.sourceFiles && report.sourceFiles.length > 0) {{
                var langs = report.sourceFiles.map(function(f) {{ return f.language + ' (' + f.count + ')'; }}).join(', ');
                list.appendChild(makeCheckItem(true, 'Source files detected', langs));
            }} else {{
                list.appendChild(makeCheckItem(false, 'No source files detected', 'Select a directory with source code'));
            }}

            // Git
            list.appendChild(makeCheckItem(report.hasGit, 'Git repository', report.hasGit ? 'Change tracking available' : 'Not a Git repo (optional)'));

            // Ollama
            if (report.ollamaRunning) {{
                var models = report.ollamaModels.length > 0 ? report.ollamaModels.join(', ') : 'No models installed';
                list.appendChild(makeCheckItem(true, 'Ollama running', models));
            }} else {{
                list.appendChild(makeCheckItem(null, 'Ollama not detected', 'Needed for fully offline operation'));
            }}

            // pdftotext
            list.appendChild(makeCheckItem(report.hasPdftotext, 'pdftotext available', report.hasPdftotext ? 'Document processing ready' : 'Install poppler-utils for PDF support'));

            // API Key
            list.appendChild(makeCheckItem(report.hasGeminiKey, report.geminiKeyEnv, report.hasGeminiKey ? 'Set' : 'Not set (needed for Gemini cloud)'));

            // System resources footer
            var footer = document.createElement('div');
            footer.className = 'flex gap-4 text-xs text-text-muted dark:text-slate-400 pt-2 border-t border-surface-3/30 dark:border-slate-700 mt-2';
            var ram = document.createElement('span');
            ram.textContent = 'RAM: ' + report.availableRamMb + ' MB';
            var disk = document.createElement('span');
            disk.textContent = 'Disk: ' + report.availableDiskMb + ' MB free';
            var wtype = document.createElement('span');
            wtype.textContent = 'Type: ' + report.workspaceType;
            footer.appendChild(ram);
            footer.appendChild(disk);
            footer.appendChild(wtype);
            list.appendChild(footer);

            container.appendChild(list);
        }})
        .catch(function(err) {{
            container.textContent = '';
            var p = document.createElement('p');
            p.className = 'text-sm text-accent-red';
            p.textContent = 'Detection failed: ' + err;
            container.appendChild(p);
        }});

    function makeCheckItem(status, label, detail) {{
        var icon, color;
        if (status === true) {{ icon = '\u2713'; color = 'text-accent-green'; }}
        else if (status === false) {{ icon = '\u2717'; color = 'text-accent-red'; }}
        else {{ icon = '\u26A0'; color = 'text-accent-orange'; }}

        var row = document.createElement('div');
        row.className = 'flex items-start gap-2 py-1.5 px-3 rounded-lg bg-surface-2/30 dark:bg-slate-700/30';

        var iconSpan = document.createElement('span');
        iconSpan.className = color + ' text-sm mt-0.5';
        iconSpan.textContent = icon;
        row.appendChild(iconSpan);

        var textDiv = document.createElement('div');
        var labelDiv = document.createElement('div');
        labelDiv.className = 'text-sm text-text-primary dark:text-slate-100';
        labelDiv.textContent = label;
        var detailDiv = document.createElement('div');
        detailDiv.className = 'text-xs text-text-muted dark:text-slate-400';
        detailDiv.textContent = detail;
        textDiv.appendChild(labelDiv);
        textDiv.appendChild(detailDiv);
        row.appendChild(textDiv);

        return row;
    }}
}})();
"#, workspace_json = serde_json::to_string(&workspace).unwrap_or_else(|_| "null".to_owned()))))
        }
    }
}
