use std::collections::HashMap;

use maud::{Markup, html};

use super::back_button;

/// Step 5: Summary and Start button.
pub(crate) fn render(params: &HashMap<String, String>) -> Markup {
    let workspace = params.get("workspace_path").cloned().unwrap_or_default();
    let provider = params
        .get("provider")
        .cloned()
        .unwrap_or_else(|| "cloud".to_owned());
    let enable_batch = params
        .get("enable_batch")
        .map(|v| v == "true")
        .unwrap_or(false);
    let enable_continuous = params
        .get("enable_continuous")
        .map(|v| v == "true")
        .unwrap_or(false);
    let api_key_env = params
        .get("api_key_env")
        .cloned()
        .unwrap_or_else(|| "GEMINI_API_KEY".to_owned());

    let provider_label = match provider.as_str() {
        "cloud" | "gemini" => "Gemini Flash (Cloud)",
        "local" | "qwen3_local" => "Ollama / Local (Offline)",
        "mock" => "Mock (Testing)",
        _ => "Auto",
    };

    html! {
        div class="space-y-5" {
            h2 class="text-lg font-semibold text-text-primary dark:text-slate-100" {
                "Ready to Start"
            }

            p class="text-sm text-text-secondary dark:text-slate-300" {
                "Review your settings and start AETHER."
            }

            // Hidden fields
            input type="hidden" name="workspace_path" value=(workspace) {}
            input type="hidden" name="provider" value=(provider) {}
            input type="hidden" name="api_key_env" value=(api_key_env) {}
            @if enable_batch {
                input type="hidden" name="enable_batch" value="true" {}
            }
            @if enable_continuous {
                input type="hidden" name="enable_continuous" value="true" {}
            }

            // Summary card
            div class="bg-surface-2/30 dark:bg-slate-700/30 rounded-lg divide-y divide-surface-3/30 dark:divide-slate-600" {
                (summary_row("Workspace", &workspace))
                (summary_row("Inference", provider_label))
                @if provider == "cloud" {
                    (summary_row("API Key", &api_key_env))
                }
                (summary_row("Batch Pipeline", if enable_batch { "Enabled" } else { "Disabled" }))
                (summary_row("Drift Monitor", if enable_continuous { "Enabled" } else { "Disabled" }))
            }

            // Estimate (filled by JS if Tauri available)
            div id="estimate-container" class="text-sm text-text-muted dark:text-slate-400" {}

            // Status message (shown after clicking Start)
            div id="wizard-status" class="hidden text-center py-4" {
                div class="text-sm text-accent-cyan" { "Setting up AETHER\u{2026}" }
                div class="text-xs text-text-muted dark:text-slate-400 mt-1" {
                    "Writing configuration and restarting."
                }
            }

            // Navigation
            div id="wizard-nav" class="flex justify-between pt-4" {
                (back_button(4, params))

                button
                    type="button"
                    id="start-btn"
                    class="px-6 py-2.5 bg-accent-cyan text-white rounded-lg hover:bg-accent-cyan/90 transition-colors font-medium"
                    onclick="startAether()"
                {
                    "Start AETHER \u{2192}"
                }
            }
        }

        // Start AETHER logic
        script {
            (maud::PreEscaped(format!(r#"
function startAether() {{
    if (!window.__TAURI__) {{
        alert('Requires AETHER Desktop');
        return;
    }}
    var btn = document.getElementById('start-btn');
    btn.disabled = true;
    btn.textContent = 'Starting\u2026';
    btn.className = btn.className.replace('hover:bg-accent-cyan/90', '') + ' opacity-50 cursor-not-allowed';
    document.getElementById('wizard-status').classList.remove('hidden');

    var config = {{
        workspacePath: {workspace_json},
        provider: {provider_json},
        model: null,
        endpoint: null,
        apiKeyEnv: {api_key_env_json},
        enableBatch: {enable_batch},
        enableContinuous: {enable_continuous}
    }};
    window.__TAURI__.core.invoke('finalize_wizard', {{ config: config }})
        .catch(function(err) {{
            document.getElementById('wizard-status').classList.add('hidden');
            btn.disabled = false;
            btn.textContent = 'Start AETHER \u2192';
            var errP = document.createElement('p');
            errP.className = 'text-sm text-accent-red mt-2';
            errP.textContent = 'Setup failed: ' + err;
            document.getElementById('wizard-nav').appendChild(errP);
        }});
}}
"#,
                workspace_json = serde_json::to_string(&workspace).unwrap_or_else(|_| "\"\"".to_owned()),
                provider_json = serde_json::to_string(&provider).unwrap_or_else(|_| "\"cloud\"".to_owned()),
                api_key_env_json = serde_json::to_string(&api_key_env).unwrap_or_else(|_| "\"GEMINI_API_KEY\"".to_owned()),
                enable_batch = enable_batch,
                enable_continuous = enable_continuous,
            )))
        }
    }
}

/// Render a summary row.
fn summary_row(label: &str, value: &str) -> Markup {
    html! {
        div class="flex items-center justify-between px-4 py-2.5" {
            span class="text-sm text-text-muted dark:text-slate-400" { (label) }
            span class="text-sm text-text-primary dark:text-slate-100 font-mono" { (value) }
        }
    }
}
