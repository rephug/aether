use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;

use crate::support::{self, DashboardState};

pub(crate) async fn overview_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    match support::load_overview_data(state.shared.as_ref()).await {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => support::json_internal_error(err),
    }
}
