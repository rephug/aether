use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let c = &config.embeddings;
    let provider_str = c.provider.as_str();

    html! {
        form hx-post="/api/v1/settings/embeddings"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Embeddings Settings" }

            (helpers::toggle_input(
                "enabled",
                "Enabled",
                c.enabled,
                "Enable embedding generation for semantic search",
            ))

            (helpers::select_input_with_htmx(
                "provider",
                "Provider",
                provider_str,
                &[
                    ("qwen3_local", "Qwen3 Local"),
                    ("candle", "Candle (local)"),
                    ("gemini_native", "Gemini Native"),
                    ("openai_compat", "OpenAI Compatible"),
                ],
                "Embedding provider backend",
                "embeddings",
            ))

            (helpers::text_input(
                "model",
                "Model",
                c.model.as_deref().unwrap_or(""),
                "Embedding model name",
            ))

            @if provider_str == "openai_compat" {
                (helpers::text_input(
                    "endpoint",
                    "Endpoint",
                    c.endpoint.as_deref().unwrap_or(""),
                    "Embedding API endpoint",
                ))
            }

            @if provider_str == "gemini_native" || provider_str == "openai_compat" {
                (helpers::text_input(
                    "api_key_env",
                    "API Key Env",
                    c.api_key_env.as_deref().unwrap_or(""),
                    "Environment variable for embedding API key",
                ))
            }

            (helpers::select_input(
                "vector_backend",
                "Vector Backend",
                c.vector_backend.as_str(),
                &[
                    ("sqlite", "SQLite"),
                    ("lancedb", "LanceDB"),
                ],
                "Vector storage backend. Restart required to change.",
            ))

            @if let Some(dims) = c.dimensions {
                (helpers::readonly_field(
                    "Dimensions",
                    &dims.to_string(),
                    "Embedding dimensions (derived from model)",
                ))
            } @else {
                (helpers::number_input(
                    "dimensions",
                    "Dimensions",
                    "",
                    "Embedding dimensions (derived from model)",
                    Some("1"),
                    None,
                    None,
                ))
            }

            (helpers::text_input(
                "task_type",
                "Task Type",
                c.task_type.as_deref().unwrap_or(""),
                "Task type hint for embedding model",
            ))

            @if provider_str == "candle" {
                (helpers::text_input(
                    "candle.model_dir",
                    "Candle Model Directory",
                    c.candle.model_dir.as_deref().unwrap_or(""),
                    "Local model directory for Candle",
                ))
            }

            (helpers::save_reset_buttons("embeddings"))
        }
    }
}
