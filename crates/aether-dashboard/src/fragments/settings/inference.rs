use std::collections::HashMap;

use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig, params: &HashMap<String, String>) -> Markup {
    let c = &config.inference;
    let saved_provider = c.provider.as_str();
    // Use query-param override if present (from HTMX hx-include on dropdown change),
    // otherwise fall back to the saved config value.
    let provider_str = params
        .get("provider")
        .map(|s| s.as_str())
        .unwrap_or(saved_provider);

    html! {
        form hx-post="/api/v1/settings/inference"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Inference Settings" }

            (helpers::select_input_with_htmx(
                "provider",
                "Provider",
                provider_str,
                &[
                    ("auto", "Auto"),
                    ("tiered", "Tiered"),
                    ("gemini", "Gemini"),
                    ("qwen3_local", "Qwen3 Local"),
                    ("openai_compat", "OpenAI Compatible"),
                ],
                "Inference provider backend",
                "inference",
            ))

            (helpers::text_input(
                "model",
                "Model",
                c.model.as_deref().unwrap_or(""),
                "Model name for inference requests",
            ))

            @if provider_str == "openai_compat" || provider_str == "qwen3_local" {
                (helpers::text_input(
                    "endpoint",
                    "Endpoint",
                    c.endpoint.as_deref().unwrap_or(""),
                    "API endpoint URL",
                ))
            }

            (helpers::text_input(
                "api_key_env",
                "API Key Env",
                &c.api_key_env,
                "Environment variable containing the API key",
            ))

            (helpers::number_input(
                "concurrency",
                "Concurrency",
                c.concurrency,
                "Maximum concurrent inference requests",
                Some("1"),
                Some("24"),
                None,
            ))

            @if provider_str == "tiered" {
                (render_tiered_section(c.tiered.as_ref()))
            }

            (helpers::save_reset_buttons("inference"))
        }
    }
}

fn render_tiered_section(tiered: Option<&aether_config::TieredConfig>) -> Markup {
    let t = match tiered {
        Some(t) => t,
        None => {
            return html! {
                p class="text-xs text-text-muted italic" {
                    "Tiered configuration not set. Save with Tiered provider to initialize defaults."
                }
            };
        }
    };

    let content = html! {
        (helpers::text_input(
            "tiered.primary",
            "Primary Provider",
            &t.primary,
            "Primary inference provider name",
        ))

        (helpers::text_input(
            "tiered.primary_model",
            "Primary Model",
            t.primary_model.as_deref().unwrap_or(""),
            "Model for the primary provider",
        ))

        (helpers::text_input(
            "tiered.primary_endpoint",
            "Primary Endpoint",
            t.primary_endpoint.as_deref().unwrap_or(""),
            "Endpoint for the primary provider",
        ))

        (helpers::text_input(
            "tiered.primary_api_key_env",
            "Primary API Key Env",
            &t.primary_api_key_env,
            "Environment variable for primary provider API key",
        ))

        (helpers::slider_input(
            "tiered.primary_threshold",
            "Primary Threshold",
            t.primary_threshold,
            0.0,
            1.0,
            0.01,
            "Confidence threshold for using the primary provider",
        ))

        (helpers::text_input(
            "tiered.fallback_model",
            "Fallback Model",
            t.fallback_model.as_deref().unwrap_or(""),
            "Model for fallback inference",
        ))

        (helpers::text_input(
            "tiered.fallback_endpoint",
            "Fallback Endpoint",
            t.fallback_endpoint.as_deref().unwrap_or(""),
            "Endpoint for the fallback provider",
        ))

        (helpers::toggle_input(
            "tiered.retry_with_fallback",
            "Retry with Fallback",
            t.retry_with_fallback,
            "Automatically retry failed requests using the fallback provider",
        ))
    };

    helpers::collapsible_section("tiered-config", "Tiered Configuration", true, content)
}
