use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::support::DashboardState;

pub(crate) mod anatomy;
mod architecture;
mod blast_radius;
pub(crate) mod catalog;
mod causal_chain;
pub(crate) mod common;
pub(crate) mod coupling;
pub(crate) mod drift;
pub(crate) mod file;
pub(crate) mod flow;
pub(crate) mod glossary;
mod graph;
pub(crate) mod health;
mod overview;
mod search;
pub(crate) mod symbol;
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
        .route("/api/v1/overview", get(overview::overview_handler))
        .route("/api/v1/search", get(search::search_handler))
        .route("/api/v1/graph", get(graph::graph_handler))
        .route("/api/v1/drift", get(drift::drift_handler))
        .route("/api/v1/coupling", get(coupling::coupling_handler))
        .route("/api/v1/health", get(health::health_handler))
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
}
