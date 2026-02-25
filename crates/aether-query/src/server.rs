use std::sync::Arc;
use std::time::Duration;

use aether_core::normalize_path;
use aether_mcp::{AetherMcpServer, SharedState};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::QueryConfig;
use crate::health::{
    StalenessInfo, compute_staleness, current_unix_timestamp, read_last_indexed_at,
};

#[derive(Clone)]
struct AppState {
    mcp_http_service: StreamableHttpService<AetherMcpServer>,
    mcp_server: AetherMcpServer,
    shared_state: Arc<SharedState>,
    query_config: QueryConfig,
    semaphore: Arc<Semaphore>,
}

#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

#[derive(Debug, Serialize)]
struct ErrorBodyWithMessage<'a> {
    error: &'a str,
    message: String,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    schema_version: u32,
    stale: bool,
    last_indexed_at: Option<i64>,
    staleness_minutes: Option<u64>,
    warning: Option<String>,
}

#[derive(Debug, Serialize)]
struct InfoResponse {
    aether_query_version: &'static str,
    index_path: String,
    backend: String,
    symbols: i64,
    sir_count: i64,
    schema_version: u32,
    read_only: bool,
}

pub fn build_router(
    shared_state: Arc<SharedState>,
    mcp_server: AetherMcpServer,
    query_config: QueryConfig,
) -> Router {
    let mcp_http_service = build_mcp_http_service(mcp_server.clone());
    let max_concurrent = query_config.query.max_concurrent_queries.max(1);

    let state = Arc::new(AppState {
        mcp_http_service,
        mcp_server,
        shared_state,
        query_config,
        semaphore: Arc::new(Semaphore::new(max_concurrent)),
    });

    Router::new()
        .route("/mcp", post(mcp_handler))
        .route("/health", get(health_handler))
        .route("/info", get(info_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn build_mcp_http_service(server: AetherMcpServer) -> StreamableHttpService<AetherMcpServer> {
    let server_template = server;
    let session_manager = Arc::new(LocalSessionManager::default());
    let config = StreamableHttpServerConfig {
        stateful_mode: false,
        ..Default::default()
    };

    StreamableHttpService::new(move || Ok(server_template.clone()), session_manager, config)
}

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if state.query_config.query.auth_token.is_empty() {
        return next.run(request).await;
    }

    if is_bearer_authorized(request.headers(), &state.query_config.query.auth_token) {
        next.run(request).await
    } else {
        json_error(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}

fn is_bearer_authorized(headers: &HeaderMap, expected_token: &str) -> bool {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    token == expected_token
}

async fn mcp_handler(State(state): State<Arc<AppState>>, request: Request) -> Response {
    let _permit = match state.semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return json_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
    };

    let timeout_ms = state.query_config.query.read_timeout_ms;
    let mcp_response = match timeout(
        Duration::from_millis(timeout_ms),
        state.mcp_http_service.handle(request),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => return json_error(StatusCode::GATEWAY_TIMEOUT, "query_timeout"),
    };

    mcp_response.into_response()
}

async fn health_handler(State(state): State<Arc<AppState>>) -> Response {
    match build_health_response(&state) {
        Ok(response) => Json(response).into_response(),
        Err(err) => json_error_with_message(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            err.to_string(),
        ),
    }
}

async fn info_handler(State(state): State<Arc<AppState>>) -> Response {
    let status = match state.mcp_server.aether_status_logic() {
        Ok(status) => status,
        Err(err) => {
            return json_error_with_message(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                err.to_string(),
            );
        }
    };

    let response = InfoResponse {
        aether_query_version: env!("CARGO_PKG_VERSION"),
        index_path: normalize_path(&state.query_config.query.index_path.to_string_lossy()),
        backend: state
            .shared_state
            .config
            .storage
            .graph_backend
            .as_str()
            .to_owned(),
        symbols: status.symbol_count,
        sir_count: status.sir_count,
        schema_version: state.shared_state.schema_version.version,
        read_only: state.shared_state.read_only,
    };

    Json(response).into_response()
}

fn build_health_response(
    state: &AppState,
) -> Result<HealthResponse, Box<dyn std::error::Error + Send + Sync>> {
    let last_indexed_at = read_last_indexed_at(&state.query_config.query.index_path)?;
    let staleness = compute_staleness(
        current_unix_timestamp(),
        last_indexed_at,
        state.query_config.staleness.warn_after_minutes,
    );
    Ok(HealthResponse::from_staleness(
        state.shared_state.schema_version.version,
        staleness,
    ))
}

impl HealthResponse {
    fn from_staleness(schema_version: u32, staleness: StalenessInfo) -> Self {
        Self {
            status: if staleness.stale { "warn" } else { "ok" },
            schema_version,
            stale: staleness.stale,
            last_indexed_at: staleness.last_indexed_at,
            staleness_minutes: staleness.staleness_minutes,
            warning: staleness.warning,
        }
    }
}

fn json_error(status: StatusCode, code: &'static str) -> Response {
    (status, Json(ErrorBody { error: code })).into_response()
}

fn json_error_with_message(status: StatusCode, code: &'static str, message: String) -> Response {
    (
        status,
        Json(ErrorBodyWithMessage {
            error: code,
            message,
        }),
    )
        .into_response()
}
