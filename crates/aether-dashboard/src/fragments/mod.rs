use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};

use crate::support::DashboardState;

mod anatomy;
mod architecture;
mod ask;
mod autopsy;
mod blast_radius;
mod causal;
mod changes;
mod context;
mod coupling;
mod decompose;
mod drift_table;
mod file;
mod flow;
mod glossary;
mod graph;
mod health;
mod health_score;
mod overview;
mod prompts;
mod search_results;
mod spec;
mod symbol;
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
}
