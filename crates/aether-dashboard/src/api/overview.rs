use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;

use crate::support::{self, DashboardState};

pub(crate) async fn overview_handler(
    State(state): State<Arc<DashboardState>>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_async_with_timeout(move || async move {
        support::load_overview_data(shared.as_ref()).await
    })
    .await
    {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => {
            if let Some(message) = support::extract_timeout_error_message(err.as_str()) {
                support::json_timeout_error(message)
            } else {
                support::json_internal_error(err)
            }
        }
    }
}
