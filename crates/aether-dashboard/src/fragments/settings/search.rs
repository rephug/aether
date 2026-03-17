use std::collections::HashMap;

use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig, params: &HashMap<String, String>) -> Markup {
    let c = &config.search;
    let saved_reranker = c.reranker.as_str();
    let reranker_str = params
        .get("reranker")
        .map(|s| s.as_str())
        .unwrap_or(saved_reranker);

    html! {
        form hx-post="/api/v1/settings/search"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Search Settings" }

            (helpers::select_input_with_htmx(
                "reranker",
                "Reranker",
                reranker_str,
                &[
                    ("none", "None"),
                    ("candle", "Candle (local)"),
                    ("cohere", "Cohere"),
                ],
                "Reranker for search result refinement",
                "search",
            ))

            (helpers::number_input(
                "rerank_window",
                "Rerank Window",
                c.rerank_window,
                "Number of candidates to pass through reranker",
                Some("1"),
                None,
                None,
            ))

            (helpers::section_divider("Similarity Thresholds"))

            (helpers::slider_input(
                "thresholds.default",
                "Default Threshold",
                c.thresholds.default as f64,
                0.30,
                0.95,
                0.01,
                "Default similarity threshold for semantic search",
            ))

            (helpers::slider_input(
                "thresholds.rust",
                "Rust Threshold",
                c.thresholds.rust as f64,
                0.30,
                0.95,
                0.01,
                "Similarity threshold for Rust files",
            ))

            (helpers::slider_input(
                "thresholds.typescript",
                "TypeScript Threshold",
                c.thresholds.typescript as f64,
                0.30,
                0.95,
                0.01,
                "Similarity threshold for TypeScript files",
            ))

            (helpers::slider_input(
                "thresholds.python",
                "Python Threshold",
                c.thresholds.python as f64,
                0.30,
                0.95,
                0.01,
                "Similarity threshold for Python files",
            ))

            @if reranker_str == "candle" {
                (helpers::section_divider("Candle Reranker"))
                (helpers::text_input(
                    "candle.model_dir",
                    "Model Directory",
                    c.candle.model_dir.as_deref().unwrap_or(""),
                    "Local model directory for Candle reranker",
                ))
            }

            @if reranker_str == "cohere" {
                (helpers::section_divider("Cohere Reranker"))
                (helpers::text_input(
                    "providers.cohere.api_key_env",
                    "Cohere API Key Env",
                    &config.providers.cohere.api_key_env,
                    "Environment variable for Cohere API key",
                ))
            }

            (helpers::save_reset_buttons("search"))
        }
    }
}
