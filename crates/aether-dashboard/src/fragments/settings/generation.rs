use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let c = &config.sir_quality;
    html! {
        form hx-post="/api/v1/settings/generation"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "SIR Generation Settings" }

            (helpers::collapsible_section("triage-pass", "Triage Pass", true, render_triage(c)))
            (helpers::collapsible_section("deep-pass", "Deep Pass", true, render_deep(c)))

            (helpers::save_reset_buttons("generation"))
        }
    }
}

fn render_triage(c: &aether_config::SirQualityConfig) -> Markup {
    html! {
        (helpers::toggle_input(
            "triage_pass",
            "Triage Pass",
            c.triage_pass,
            "Enable the triage quality pass during indexing",
        ))

        (helpers::text_input(
            "triage_provider",
            "Triage Provider",
            c.triage_provider.as_deref().unwrap_or(""),
            "Inference provider for triage pass",
        ))

        (helpers::text_input(
            "triage_model",
            "Triage Model",
            c.triage_model.as_deref().unwrap_or(""),
            "Model for triage pass",
        ))

        (helpers::text_input(
            "triage_endpoint",
            "Triage Endpoint",
            c.triage_endpoint.as_deref().unwrap_or(""),
            "Endpoint for triage inference",
        ))

        (helpers::text_input(
            "triage_api_key_env",
            "Triage API Key Env",
            c.triage_api_key_env.as_deref().unwrap_or(""),
            "API key env var for triage provider",
        ))

        (helpers::slider_input(
            "triage_priority_threshold",
            "Triage Priority Threshold",
            c.triage_priority_threshold,
            0.0,
            1.0,
            0.01,
            "Minimum priority score to trigger triage",
        ))

        (helpers::slider_input(
            "triage_confidence_threshold",
            "Triage Confidence Threshold",
            c.triage_confidence_threshold,
            0.0,
            1.0,
            0.01,
            "Minimum confidence score to accept triage result",
        ))

        (helpers::number_input(
            "triage_max_symbols",
            "Max Symbols",
            c.triage_max_symbols,
            "Max symbols per triage batch (0 = unlimited)",
            Some("0"),
            None,
            None,
        ))

        (helpers::number_input(
            "triage_concurrency",
            "Concurrency",
            c.triage_concurrency,
            "Concurrent triage requests",
            Some("1"),
            Some("24"),
            None,
        ))

        (helpers::number_input(
            "triage_timeout_secs",
            "Timeout (seconds)",
            c.triage_timeout_secs,
            "Timeout per triage request (seconds)",
            Some("1"),
            None,
            None,
        ))
    }
}

fn render_deep(c: &aether_config::SirQualityConfig) -> Markup {
    html! {
        (helpers::toggle_input(
            "deep_pass",
            "Deep Pass",
            c.deep_pass,
            "Enable the deep quality pass for high-priority symbols",
        ))

        (helpers::text_input(
            "deep_provider",
            "Deep Provider",
            c.deep_provider.as_deref().unwrap_or(""),
            "Inference provider for deep pass",
        ))

        (helpers::text_input(
            "deep_model",
            "Deep Model",
            c.deep_model.as_deref().unwrap_or(""),
            "Model for deep pass",
        ))

        (helpers::text_input(
            "deep_endpoint",
            "Deep Endpoint",
            c.deep_endpoint.as_deref().unwrap_or(""),
            "Endpoint for deep inference",
        ))

        (helpers::text_input(
            "deep_api_key_env",
            "Deep API Key Env",
            c.deep_api_key_env.as_deref().unwrap_or(""),
            "API key env var for deep provider",
        ))

        (helpers::slider_input(
            "deep_priority_threshold",
            "Deep Priority Threshold",
            c.deep_priority_threshold,
            0.0,
            1.0,
            0.01,
            "Minimum priority score for deep analysis",
        ))

        (helpers::slider_input(
            "deep_confidence_threshold",
            "Deep Confidence Threshold",
            c.deep_confidence_threshold,
            0.0,
            1.0,
            0.01,
            "Minimum confidence to accept deep result",
        ))

        (helpers::number_input(
            "deep_max_symbols",
            "Max Symbols",
            c.deep_max_symbols,
            "Max symbols per deep batch (0 = unlimited)",
            Some("0"),
            None,
            None,
        ))

        (helpers::number_input(
            "deep_max_neighbors",
            "Max Neighbors",
            c.deep_max_neighbors,
            "Max neighbor symbols for context",
            Some("1"),
            Some("50"),
            None,
        ))

        (helpers::number_input(
            "deep_concurrency",
            "Concurrency",
            c.deep_concurrency,
            "Concurrent deep requests",
            Some("1"),
            Some("24"),
            None,
        ))

        (helpers::number_input(
            "deep_timeout_secs",
            "Timeout (seconds)",
            c.deep_timeout_secs,
            "Timeout per deep request (seconds)",
            Some("1"),
            None,
            None,
        ))
    }
}
