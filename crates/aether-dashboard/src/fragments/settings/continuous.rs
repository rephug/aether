use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let cont = match config.continuous.as_ref() {
        Some(c) => c,
        None => {
            return html! {
                div class="rounded-xl border border-surface-3/40 bg-surface-1/40 p-5 space-y-3" {
                    h3 class="text-sm font-semibold text-text-primary pb-2" { "Continuous Intelligence Settings" }
                    p class="text-sm text-text-muted" {
                        "Continuous intelligence is not configured. Add a "
                        code class="text-xs bg-surface-3/30 px-1 py-0.5 rounded" { "[continuous]" }
                        " section to your config.toml to enable staleness monitoring."
                    }
                }
            };
        }
    };

    let requeue_pass_options: &[(&str, &str)] =
        &[("triage", "Triage"), ("deep", "Deep"), ("scan", "Scan")];

    html! {
        form hx-post="/api/v1/settings/continuous"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Continuous Intelligence Settings" }

            (helpers::toggle_input(
                "enabled",
                "Enabled",
                cont.enabled,
                "Enable continuous staleness monitoring",
            ))

            (helpers::text_input(
                "schedule",
                "Schedule",
                &cont.schedule,
                "Run schedule (e.g., 'nightly', 'hourly')",
            ))

            (helpers::section_divider("Staleness Scoring"))

            (helpers::number_input(
                "staleness_half_life_days",
                "Half-Life (days)",
                cont.staleness_half_life_days,
                "Half-life in days for staleness decay",
                Some("0.1"),
                None,
                Some("0.5"),
            ))

            (helpers::number_input(
                "staleness_sigmoid_k",
                "Sigmoid K",
                cont.staleness_sigmoid_k,
                "Sigmoid steepness for staleness scoring",
                None,
                None,
                Some("0.01"),
            ))

            (helpers::slider_input(
                "neighbor_decay",
                "Neighbor Decay",
                cont.neighbor_decay,
                0.0,
                1.0,
                0.01,
                "Decay factor for neighbor staleness propagation",
            ))

            (helpers::slider_input(
                "neighbor_cutoff",
                "Neighbor Cutoff",
                cont.neighbor_cutoff,
                0.0,
                1.0,
                0.01,
                "Minimum score to propagate to neighbors",
            ))

            (helpers::slider_input(
                "coupling_predict_threshold",
                "Coupling Predict Threshold",
                cont.coupling_predict_threshold,
                0.0,
                1.0,
                0.01,
                "Coupling strength threshold for predictive staleness",
            ))

            (helpers::section_divider("Requeue"))

            (helpers::number_input(
                "max_requeue_per_run",
                "Max Requeue per Run",
                cont.max_requeue_per_run,
                "Maximum symbols to requeue per run (0 = unlimited)",
                Some("0"),
                None,
                None,
            ))

            (helpers::toggle_input(
                "auto_submit",
                "Auto Submit",
                cont.auto_submit,
                "Automatically submit requeued symbols for re-indexing",
            ))

            (helpers::select_input(
                "requeue_pass",
                "Requeue Pass",
                &cont.requeue_pass,
                requeue_pass_options,
                "Quality pass level for requeued symbols",
            ))

            (helpers::slider_input(
                "priority_pagerank_alpha",
                "PageRank Alpha",
                cont.priority_pagerank_alpha,
                0.0,
                1.0,
                0.01,
                "PageRank damping factor for priority ranking",
            ))

            (helpers::save_reset_buttons("continuous"))
        }
    }
}
