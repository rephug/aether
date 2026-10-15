use std::sync::Arc;

use axum::extract::{Path, Request};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
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
        .layer(middleware::from_fn(reject_cross_origin_writes))
        .with_state(app_state)
}

/// Rejects state-changing requests that a browser sent from another origin.
///
/// The dashboard has no authentication, and a form POST is a CORS "simple"
/// request that browsers send cross-origin without a preflight, so without
/// this check any web page could rewrite `.aether/config.toml`. Only the
/// dashboard's own pages (`http://127.0.0.1:<port>` or
/// `http://localhost:<port>`, which is also what the desktop webview loads)
/// may write. Requests without `Origin`/`Sec-Fetch-Site` come from non-browser
/// clients such as curl or the VS Code extension and are allowed.
async fn reject_cross_origin_writes(request: Request, next: Next) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD) || is_same_origin(&request) {
        return next.run(request).await;
    }
    (StatusCode::FORBIDDEN, "cross-origin request rejected").into_response()
}

fn is_same_origin(request: &Request) -> bool {
    let headers = request.headers();
    if let Some(site) = headers.get("sec-fetch-site")
        && !matches!(site.as_bytes(), b"same-origin" | b"none")
    {
        return false;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        });
    let (Ok(origin), Some(host)) = (origin.to_str(), host) else {
        return false;
    };
    let Some(origin_authority) = origin.strip_prefix("http://") else {
        return false;
    };
    // Require a loopback name so a DNS-rebound hostname cannot pass as the
    // dashboard, and the Host port so other local servers cannot either.
    let origin_name = origin_authority
        .rsplit_once(':')
        .map_or(origin_authority, |(name, _port)| name);
    (origin_name == "127.0.0.1" || origin_name.eq_ignore_ascii_case("localhost"))
        && origin_authority.eq_ignore_ascii_case(host)
}

/// Stateless wizard-only router — serves the setup wizard UI without
/// requiring a `SharedState` (no SQLite store, no graph, no config).
pub fn wizard_router() -> Router {
    Router::new()
        .route("/dashboard", get(wizard_shell))
        .route("/dashboard/", get(wizard_shell))
        .route("/dashboard/static/{*path}", get(wizard_static))
        .merge(fragments::wizard::wizard_fragment_router())
        .route("/dashboard/{*path}", get(wizard_shell_catchall))
}

async fn wizard_shell() -> Response {
    embedded_file_response("wizard.html")
}

async fn wizard_shell_catchall(Path(_path): Path<String>) -> Response {
    embedded_file_response("wizard.html")
}

async fn wizard_static(Path(path): Path<String>) -> Response {
    let normalized = path.trim_start_matches('/');
    if normalized.is_empty() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    embedded_file_response(normalized)
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
