use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::support::DashboardState;

mod overview;
mod search_results;
mod stubs;
mod symbol_detail;

pub(crate) fn fragment_router() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/dashboard/frag/overview", get(overview::overview_fragment))
        .route(
            "/dashboard/frag/search",
            get(search_results::search_fragment),
        )
        .route(
            "/dashboard/frag/symbol/{symbol_id}",
            get(symbol_detail::symbol_detail_fragment),
        )
        .route("/dashboard/frag/graph", get(stubs::graph_fragment))
        .route("/dashboard/frag/drift-table", get(stubs::drift_fragment))
        .route("/dashboard/frag/coupling", get(stubs::coupling_fragment))
        .route("/dashboard/frag/health", get(stubs::health_fragment))
}
