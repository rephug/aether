use std::sync::Arc;

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, extract::State};
use rust_embed::Embed;

mod api;
mod fragments;
pub mod narrative;
mod state;
mod support;
pub use state::SharedState;

#[derive(Embed)]
#[folder = "src/static/"]
struct StaticFiles;

pub fn dashboard_router(state: Arc<SharedState>) -> Router {
    let app_state = Arc::new(support::DashboardState::new(state));

    Router::new()
        .route("/dashboard", get(dashboard_shell))
        .route("/dashboard/", get(dashboard_shell))
        .route("/dashboard/static/{*path}", get(dashboard_static))
        .merge(fragments::fragment_router())
        .merge(api::api_router())
        .route("/dashboard/{*path}", get(dashboard_shell_fallback))
        .with_state(app_state)
}

async fn dashboard_shell(State(_state): State<Arc<support::DashboardState>>) -> Response {
    embedded_file_response("index.html")
}

async fn dashboard_shell_fallback(
    State(_state): State<Arc<support::DashboardState>>,
    Path(_path): Path<String>,
) -> Response {
    embedded_file_response("index.html")
}

async fn dashboard_static(
    State(_state): State<Arc<support::DashboardState>>,
    Path(path): Path<String>,
) -> Response {
    let normalized = path.trim_start_matches('/');
    if normalized.is_empty() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    embedded_file_response(normalized)
}

fn embedded_file_response(path: &str) -> Response {
    let Some(file) = StaticFiles::get(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    support::embedded_bytes_response(file.data.into_owned(), mime.essence_str())
}

#[cfg(test)]
mod tests;
