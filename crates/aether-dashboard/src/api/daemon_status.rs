use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::support::DashboardState;

#[derive(Serialize)]
struct DaemonStatusResponse {
    daemon: bool,
    pid: u32,
    port: u16,
    workspace: String,
    uptime_seconds: u64,
}

pub(crate) async fn daemon_status_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let elapsed = state.started_at.elapsed().as_secs();
    Json(DaemonStatusResponse {
        daemon: true,
        pid: std::process::id(),
        port: state.shared.config.dashboard.port,
        workspace: state.shared.workspace.display().to_string(),
        uptime_seconds: elapsed,
    })
}
