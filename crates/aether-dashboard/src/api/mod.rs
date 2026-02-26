use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::support::DashboardState;

mod overview;
mod search;
mod stubs;

pub(crate) fn api_router() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/api/v1/overview", get(overview::overview_handler))
        .route("/api/v1/search", get(search::search_handler))
        .route("/api/v1/graph", get(stubs::graph_handler))
        .route("/api/v1/drift", get(stubs::drift_handler))
        .route("/api/v1/coupling", get(stubs::coupling_handler))
        .route("/api/v1/health", get(stubs::health_handler))
}
