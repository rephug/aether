use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let c = &config.drift;
    html! {
        form hx-post="/api/v1/settings/drift"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Drift Detection Settings" }

            (helpers::toggle_input(
                "enabled",
                "Enabled",
                c.enabled,
                "Enable drift detection between code and SIR documentation",
            ))

            (helpers::slider_input(
                "drift_threshold",
                "Drift Threshold",
                c.drift_threshold as f64,
                0.0,
                1.0,
                0.01,
                "Similarity threshold below which drift is flagged",
            ))

            (helpers::text_input(
                "analysis_window",
                "Analysis Window",
                &c.analysis_window,
                "Commit range for drift analysis (e.g., '100 commits')",
            ))

            (helpers::toggle_input(
                "auto_analyze",
                "Auto Analyze",
                c.auto_analyze,
                "Automatically analyze drift after indexing",
            ))

            (helpers::number_input(
                "hub_percentile",
                "Hub Percentile",
                c.hub_percentile,
                "Percentile cutoff for hub symbol detection",
                Some("1"),
                Some("100"),
                None,
            ))

            (helpers::save_reset_buttons("drift"))
        }
    }
}
