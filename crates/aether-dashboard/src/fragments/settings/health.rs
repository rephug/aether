use aether_config::AetherConfig;
use maud::{Markup, html};

use super::helpers;

pub(crate) fn render(config: &AetherConfig) -> Markup {
    let h = &config.health;
    let p = &config.planner;
    let hs = &config.health_score;

    // --- Risk Weights subsection ---
    let risk_weights_content = html! {
        (helpers::toggle_input(
            "health.enabled",
            "Enabled",
            h.enabled,
            "Enable health analysis",
        ))

        (helpers::slider_input(
            "health.risk_weights.pagerank",
            "PageRank Weight",
            h.risk_weights.pagerank,
            0.0,
            1.0,
            0.01,
            "Weight for PageRank centrality in risk scoring",
        ))

        (helpers::slider_input(
            "health.risk_weights.test_gap",
            "Test Gap Weight",
            h.risk_weights.test_gap,
            0.0,
            1.0,
            0.01,
            "Weight for test coverage gaps",
        ))

        (helpers::slider_input(
            "health.risk_weights.drift",
            "Drift Weight",
            h.risk_weights.drift,
            0.0,
            1.0,
            0.01,
            "Weight for documentation drift",
        ))

        (helpers::slider_input(
            "health.risk_weights.no_sir",
            "No SIR Weight",
            h.risk_weights.no_sir,
            0.0,
            1.0,
            0.01,
            "Weight for missing SIR records",
        ))

        (helpers::slider_input(
            "health.risk_weights.recency",
            "Recency Weight",
            h.risk_weights.recency,
            0.0,
            1.0,
            0.01,
            "Weight for recent change frequency",
        ))

        p class="text-xs text-text-muted italic pt-1" {
            "Risk weights are normalized to sum to 1.0"
        }
    };

    // --- Planner subsection ---
    let planner_content = html! {
        (helpers::slider_input(
            "planner.semantic_rescue_threshold",
            "Semantic Rescue Threshold",
            p.semantic_rescue_threshold as f64,
            0.30,
            0.95,
            0.01,
            "Embedding similarity threshold for semantic rescue",
        ))

        (helpers::number_input(
            "planner.semantic_rescue_max_k",
            "Semantic Rescue Max K",
            p.semantic_rescue_max_k,
            "Maximum rescued items per community",
            Some("1"),
            Some("10"),
            None,
        ))

        (helpers::slider_input(
            "planner.community_resolution",
            "Community Resolution",
            p.community_resolution,
            0.1,
            3.0,
            0.1,
            "Louvain community detection resolution parameter",
        ))

        (helpers::number_input(
            "planner.min_community_size",
            "Min Community Size",
            p.min_community_size,
            "Minimum symbols per community",
            Some("1"),
            Some("20"),
            None,
        ))
    };

    // --- Structural Thresholds subsection ---
    let structural_content = html! {
        // File Size
        (helpers::section_divider("File Size"))

        (helpers::number_input(
            "health_score.file_loc_warn",
            "LOC Warning",
            hs.file_loc_warn,
            "Lines of code warning threshold",
            Some("1"),
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.file_loc_fail",
            "LOC Failure",
            hs.file_loc_fail,
            "Lines of code failure threshold",
            Some("1"),
            None,
            None,
        ))

        // Trait Methods
        (helpers::section_divider("Trait Methods"))

        (helpers::number_input(
            "health_score.trait_method_warn",
            "Method Count Warning",
            hs.trait_method_warn,
            "Trait method count warning threshold",
            None,
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.trait_method_fail",
            "Method Count Failure",
            hs.trait_method_fail,
            "Trait method count failure threshold",
            None,
            None,
            None,
        ))

        // Internal Dependencies
        (helpers::section_divider("Internal Dependencies"))

        (helpers::number_input(
            "health_score.internal_dep_warn",
            "Dependency Count Warning",
            hs.internal_dep_warn,
            "Internal dependency count warning",
            None,
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.internal_dep_fail",
            "Dependency Count Failure",
            hs.internal_dep_fail,
            "Internal dependency count failure",
            None,
            None,
            None,
        ))

        // Dead Features
        (helpers::section_divider("Dead Features"))

        (helpers::number_input(
            "health_score.dead_feature_warn",
            "Dead Feature Warning",
            hs.dead_feature_warn,
            "Dead feature gate count warning threshold",
            Some("1"),
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.dead_feature_fail",
            "Dead Feature Failure",
            hs.dead_feature_fail,
            "Dead feature gate count failure threshold",
            Some("1"),
            None,
            None,
        ))

        // Stale References
        (helpers::section_divider("Stale References"))

        (helpers::number_input(
            "health_score.stale_ref_warn",
            "Stale Ref Warning",
            hs.stale_ref_warn,
            "Stale reference count warning threshold",
            Some("1"),
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.stale_ref_fail",
            "Stale Ref Failure",
            hs.stale_ref_fail,
            "Stale reference count failure threshold",
            Some("1"),
            None,
            None,
        ))

        // TODO Density
        (helpers::section_divider("TODO Density"))

        (helpers::number_input(
            "health_score.todo_density_warn",
            "TODO Density Warning",
            hs.todo_density_warn as f64,
            "TODO comments per 1000 lines warning",
            None,
            None,
            Some("0.1"),
        ))

        (helpers::number_input(
            "health_score.todo_density_fail",
            "TODO Density Failure",
            hs.todo_density_fail as f64,
            "TODO comments per 1000 lines failure",
            None,
            None,
            Some("0.1"),
        ))

        // Git Metrics
        (helpers::section_divider("Git Metrics"))

        (helpers::number_input(
            "health_score.churn_30d_high",
            "30-Day Churn High",
            hs.churn_30d_high,
            "30-day file churn high watermark",
            None,
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.churn_90d_high",
            "90-Day Churn High",
            hs.churn_90d_high,
            "90-day file churn high watermark",
            None,
            None,
            None,
        ))

        (helpers::number_input(
            "health_score.author_count_high",
            "Author Count High",
            hs.author_count_high,
            "Author count high watermark",
            None,
            None,
            None,
        ))

        // Semantic Metrics
        (helpers::section_divider("Semantic Metrics"))

        (helpers::slider_input(
            "health_score.drift_density_high",
            "Drift Density High",
            hs.drift_density_high as f64,
            0.0,
            1.0,
            0.01,
            "Drift density high threshold",
        ))

        (helpers::slider_input(
            "health_score.stale_sir_high",
            "Stale SIR High",
            hs.stale_sir_high as f64,
            0.0,
            1.0,
            0.01,
            "Stale SIR ratio high threshold",
        ))

        (helpers::slider_input(
            "health_score.test_gap_high",
            "Test Gap High",
            hs.test_gap_high as f64,
            0.0,
            1.0,
            0.01,
            "Test gap ratio high threshold",
        ))

        (helpers::slider_input(
            "health_score.boundary_leakage_high",
            "Boundary Leakage High",
            hs.boundary_leakage_high as f64,
            0.0,
            1.0,
            0.01,
            "Boundary leakage ratio high threshold",
        ))
    };

    html! {
        form hx-post="/api/v1/settings/health"
             hx-target="#settings-status"
             hx-swap="innerHTML"
             class="space-y-4 rounded-xl border border-surface-3/40 bg-surface-1/40 p-5"
        {
            h3 class="text-sm font-semibold text-text-primary pb-2" { "Health & Scoring Settings" }

            (helpers::collapsible_section(
                "risk-weights",
                "Risk Weights",
                true,
                risk_weights_content,
            ))

            (helpers::collapsible_section(
                "planner",
                "Planner",
                true,
                planner_content,
            ))

            (helpers::collapsible_section(
                "structural-thresholds",
                "Structural Thresholds",
                false,
                structural_content,
            ))

            (helpers::save_reset_buttons("health"))
        }
    }
}
