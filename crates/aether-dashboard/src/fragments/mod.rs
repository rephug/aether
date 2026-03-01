use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::support::DashboardState;

mod architecture;
mod blast_radius;
mod causal;
mod coupling;
mod drift_table;
mod graph;
mod health;
mod overview;
mod search_results;
mod symbol_detail;
mod time_machine;
mod xray;

pub(crate) fn fragment_router() -> Router<Arc<DashboardState>> {
    Router::new()
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
        .route(
            "/dashboard/frag/symbol/{symbol_id}",
            get(symbol_detail::symbol_detail_fragment),
        )
        .route("/dashboard/frag/graph", get(graph::graph_fragment))
        .route(
            "/dashboard/frag/drift-table",
            get(drift_table::drift_fragment),
        )
        .route("/dashboard/frag/coupling", get(coupling::coupling_fragment))
        .route("/dashboard/frag/health", get(health::health_fragment))
}
