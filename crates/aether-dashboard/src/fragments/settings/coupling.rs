use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let c = &config.coupling;
    html! {
        form hx-post="/api/v1/settings/coupling"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Coupling Settings" }

            (helpers::toggle_input(
                "enabled",
                "Enabled",
                c.enabled,
                "Enable coupling analysis via co-change mining",
            ))

            (helpers::number_input(
                "commit_window",
                "Commit Window",
                c.commit_window,
                "Number of recent commits to analyze for co-change patterns",
                Some("1"),
                None,
                None,
            ))

            (helpers::number_input(
                "min_co_change_count",
                "Min Co-Change Count",
                c.min_co_change_count,
                "Minimum number of co-changes to establish coupling",
                Some("1"),
                None,
                None,
            ))

            (helpers::number_input(
                "bulk_commit_threshold",
                "Bulk Commit Threshold",
                c.bulk_commit_threshold,
                "Commits touching more than this many files are excluded",
                Some("1"),
                None,
                None,
            ))

            (helpers::section_divider("Coupling Weights"))

            (helpers::slider_input(
                "temporal_weight",
                "Temporal Weight",
                c.temporal_weight as f64,
                0.0,
                1.0,
                0.01,
                "Weight for temporal (co-change) coupling signal",
            ))

            (helpers::slider_input(
                "static_weight",
                "Static Weight",
                c.static_weight as f64,
                0.0,
                1.0,
                0.01,
                "Weight for static (import/call) coupling signal",
            ))

            (helpers::slider_input(
                "semantic_weight",
                "Semantic Weight",
                c.semantic_weight as f64,
                0.0,
                1.0,
                0.01,
                "Weight for semantic (embedding similarity) coupling signal",
            ))

            p class="text-xs text-text-muted italic" {
                "Weights are automatically normalized to sum to 1.0"
            }

            (helpers::section_divider("Exclude Patterns"))

            {
                @let patterns = c.exclude_patterns.join(", ");
                (helpers::readonly_field(
                    "Exclude Patterns",
                    &patterns,
                    "File patterns excluded from coupling analysis. Edit in config.toml.",
                ))
            }

            (helpers::save_reset_buttons("coupling"))
        }
    }
}
