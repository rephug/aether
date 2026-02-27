use std::sync::Arc;

use axum::Router;
use axum::routing::get;

use crate::support::DashboardState;

pub(crate) mod coupling;
pub(crate) mod drift;
mod graph;
pub(crate) mod health;
mod overview;
mod search;

pub(crate) fn api_router() -> Router<Arc<DashboardState>> {
    Router::new()
        .route("/api/v1/overview", get(overview::overview_handler))
        .route("/api/v1/search", get(search::search_handler))
        .route("/api/v1/graph", get(graph::graph_handler))
        .route("/api/v1/drift", get(drift::drift_handler))
        .route("/api/v1/coupling", get(coupling::coupling_handler))
        .route("/api/v1/health", get(health::health_handler))
}
