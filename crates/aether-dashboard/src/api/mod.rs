use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::support::DashboardState;

pub(crate) mod anatomy;
mod architecture;
pub(crate) mod ask;
pub(crate) mod autopsy;
pub(crate) mod batch;
mod blast_radius;
pub(crate) mod catalog;
mod causal_chain;
pub(crate) mod changes;
pub(crate) mod common;
pub(crate) mod context;
mod context_build;
pub(crate) mod context_export;
pub(crate) mod continuous;
pub(crate) mod coupling;
mod coupling_matrix;
pub(crate) mod daemon_status;
pub(crate) mod decompose;
pub(crate) mod difficulty;
pub(crate) mod drift;
mod drift_timeline;
pub(crate) mod file;
pub(crate) mod fingerprint;
pub(crate) mod flow;
pub(crate) mod glossary;
mod graph;
pub(crate) mod health;
pub(crate) mod health_score;
mod health_scorecard;
mod memory_timeline;
mod overview;
pub(crate) mod presets;
pub(crate) mod prompts;
mod search;
pub(crate) mod settings;
pub(crate) mod spec;
mod staleness_heatmap;
pub(crate) mod symbol;
pub(crate) mod task_context;
mod time_machine;
pub(crate) mod tour;
mod xray;

pub(crate) fn api_router() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/api/v1/anatomy", get(anatomy::anatomy_handler))
        .route("/api/v1/tour", get(tour::tour_handler))
        .route("/api/v1/glossary", get(glossary::glossary_handler))
        .route("/api/v1/symbol/{selector}", get(symbol::symbol_handler))
        .route("/api/v1/file/{*path}", get(file::file_handler))
        .route("/api/v1/flow", get(flow::flow_handler))
        .route("/api/v1/difficulty", get(difficulty::difficulty_handler))
        .route(
            "/api/v1/prompts/search",
            get(prompts::prompt_search_handler),
        )
        .route(
            "/api/v1/prompts/generate",
            get(prompts::prompt_generate_handler),
        )
        .route("/api/v1/spec/{selector}", get(spec::spec_handler))
        .route("/api/v1/context/{selector}", get(context::context_handler))
        .route(
            "/api/v1/decompose/{selector}",
            get(decompose::decompose_handler),
        )
        .route(
            "/api/v1/decompose/file/{*path}",
            get(decompose::decompose_file_handler),
        )
        .route("/api/v1/autopsy/{selector}", get(autopsy::autopsy_handler))
        .route("/api/v1/overview", get(overview::overview_handler))
        .route("/api/v1/changes", get(changes::changes_handler))
        .route("/api/v1/search", get(search::search_handler))
        .route("/api/v1/ask", post(ask::ask_handler))
        .route("/api/v1/graph", get(graph::graph_handler))
        .route("/api/v1/drift", get(drift::drift_handler))
        .route("/api/v1/coupling", get(coupling::coupling_handler))
        .route("/api/v1/health", get(health::health_handler))
        .route(
            "/api/v1/health-score",
            get(health_score::health_score_handler),
        )
        .route("/api/v1/xray", get(xray::xray_handler))
        .route(
            "/api/v1/blast-radius",
            get(blast_radius::blast_radius_handler),
        )
        .route(
            "/api/v1/architecture",
            get(architecture::architecture_handler),
        )
        .route(
            "/api/v1/time-machine",
            get(time_machine::time_machine_handler),
        )
        .route(
            "/api/v1/causal-chain",
            get(causal_chain::causal_chain_handler),
        )
        .route("/api/v1/batch-status", get(batch::batch_handler))
        .route(
            "/api/v1/continuous-status",
            get(continuous::continuous_handler),
        )
        .route(
            "/api/v1/task-history",
            get(task_context::task_context_handler),
        )
        .route(
            "/api/v1/context-targets",
            get(context_export::context_export_handler),
        )
        .route("/api/v1/presets", get(presets::presets_handler))
        .route("/api/v1/presets", post(presets::create_preset_handler))
        .route(
            "/api/v1/presets/{name}",
            delete(presets::delete_preset_handler),
        )
        .route(
            "/api/v1/fingerprint-summary",
            get(fingerprint::fingerprint_summary_handler),
        )
        .route(
            "/api/v1/fingerprint-history",
            get(fingerprint::fingerprint_history_handler),
        )
        .route(
            "/api/v1/settings/{section}",
            get(settings::get_section_handler).post(settings::save_section_handler),
        )
        .route(
            "/api/v1/settings/{section}/reset",
            post(settings::reset_section_handler),
        )
        // Daemon status (lightweight, no ApiEnvelope)
        .route(
            "/api/v1/daemon-status",
            get(daemon_status::daemon_status_handler),
        )
        // Phase 9.4: Enhanced Visualization APIs
        .route(
            "/api/v1/drift-timeline",
            get(drift_timeline::drift_timeline_handler),
        )
        .route(
            "/api/v1/coupling-matrix",
            get(coupling_matrix::coupling_matrix_handler),
        )
        .route(
            "/api/v1/memory-timeline",
            get(memory_timeline::memory_timeline_handler),
        )
        .route(
            "/api/v1/health-scorecard",
            get(health_scorecard::health_scorecard_handler),
        )
        .route(
            "/api/v1/staleness-heatmap",
            get(staleness_heatmap::staleness_heatmap_handler),
        )
        // Phase Repo R.5: Context Builder
        .route(
            "/api/v1/context/file-tree",
            get(context_build::file_tree_handler),
        )
        .route(
            "/api/v1/context/build",
            post(context_build::context_build_handler),
        )
}
