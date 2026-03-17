use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::support::DashboardState;

mod anatomy;
mod architecture;
mod ask;
mod autopsy;
mod batch;
mod blast_radius;
mod causal;
mod changes;
mod context;
mod context_export;
mod continuous;
mod coupling;
mod coupling_chord;
mod decompose;
mod drift_table;
mod drift_timeline;
mod file;
mod fingerprint;
mod flow;
mod glossary;
mod graph;
mod health;
mod health_score;
mod health_scorecard;
mod memory_timeline;
mod overview;
mod presets;
mod prompts;
mod search_results;
pub(crate) mod settings;
mod spec;
mod staleness_heatmap;
mod symbol;
mod task_context;
mod time_machine;
mod tour;
mod xray;

pub(crate) fn fragment_router() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/dashboard/frag/anatomy", get(anatomy::anatomy_fragment))
        .route(
            "/dashboard/frag/anatomy/layer",
            get(anatomy::anatomy_layer_fragment),
        )
        .route(
            "/dashboard/frag/anatomy/file",
            get(anatomy::anatomy_file_fragment),
        )
        .route("/dashboard/frag/tour", get(tour::tour_fragment))
        .route("/dashboard/frag/glossary", get(glossary::glossary_fragment))
        .route("/dashboard/frag/prompts", get(prompts::prompts_fragment))
        .route(
            "/dashboard/frag/prompts/search",
            get(prompts::prompt_search_fragment),
        )
        .route("/dashboard/frag/changes", get(changes::changes_fragment))
        .route("/dashboard/frag/flow", get(flow::flow_fragment))
        .route("/dashboard/frag/ask", post(ask::ask_fragment))
        .route("/dashboard/frag/spec/{selector}", get(spec::spec_fragment))
        .route(
            "/dashboard/frag/context/{selector}",
            get(context::context_fragment),
        )
        .route(
            "/dashboard/frag/decompose/{selector}",
            get(decompose::decompose_fragment),
        )
        .route(
            "/dashboard/frag/autopsy/{selector}",
            get(autopsy::autopsy_fragment),
        )
        .route("/dashboard/frag/file/{*path}", get(file::file_fragment))
        .route(
            "/dashboard/frag/symbol/{selector}",
            get(symbol::symbol_fragment),
        )
        .route("/dashboard/frag/overview", get(overview::overview_fragment))
        .route("/dashboard/frag/xray", get(xray::xray_fragment))
        .route(
            "/dashboard/frag/search",
            get(search_results::search_fragment),
        )
        .route(
            "/dashboard/frag/blast-radius",
            get(blast_radius::blast_radius_fragment),
        )
        .route(
            "/dashboard/frag/architecture",
            get(architecture::architecture_fragment),
        )
        .route(
            "/dashboard/frag/time-machine",
            get(time_machine::time_machine_fragment),
        )
        .route("/dashboard/frag/causal", get(causal::causal_fragment))
        .route("/dashboard/frag/graph", get(graph::graph_fragment))
        .route(
            "/dashboard/frag/drift-table",
            get(drift_table::drift_fragment),
        )
        .route("/dashboard/frag/coupling", get(coupling::coupling_fragment))
        .route("/dashboard/frag/health", get(health::health_fragment))
        .route(
            "/dashboard/frag/health-score",
            get(health_score::health_score_fragment),
        )
        .route("/dashboard/frag/batch", get(batch::batch_fragment))
        .route(
            "/dashboard/frag/continuous",
            get(continuous::continuous_fragment),
        )
        .route(
            "/dashboard/frag/task-context",
            get(task_context::task_context_fragment),
        )
        .route(
            "/dashboard/frag/context-export",
            get(context_export::context_export_fragment),
        )
        .route("/dashboard/frag/presets", get(presets::presets_fragment))
        .route(
            "/dashboard/frag/fingerprint",
            get(fingerprint::fingerprint_fragment),
        )
        .route("/dashboard/frag/settings", get(settings::settings_fragment))
        .route(
            "/dashboard/frag/settings/{section}",
            get(settings::settings_section_fragment),
        )
        // Phase 9.4: Enhanced Visualizations
        .route(
            "/dashboard/frag/drift-timeline",
            get(drift_timeline::drift_timeline_fragment),
        )
        .route(
            "/dashboard/frag/coupling-chord",
            get(coupling_chord::coupling_chord_fragment),
        )
        .route(
            "/dashboard/frag/memory-timeline",
            get(memory_timeline::memory_timeline_fragment),
        )
        .route(
            "/dashboard/frag/health-scorecard",
            get(health_scorecard::health_scorecard_fragment),
        )
        .route(
            "/dashboard/frag/staleness-heatmap",
            get(staleness_heatmap::staleness_heatmap_fragment),
        )
}
