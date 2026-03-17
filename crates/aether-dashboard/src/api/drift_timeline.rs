use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DriftTimelineQuery {
    pub top: Option<usize>,
    pub since: Option<String>,
}

#[derive(Debug, Serialize)]
struct DriftTimelinePoint {
    timestamp: i64,
    drift_score: f64,
    symbol_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct DriftTimelineModule {
    name: String,
    avg_score: f64,
    point_count: usize,
    series: Vec<DriftTimelinePoint>,
}

#[derive(Debug, Serialize)]
struct DriftTimelineData {
    modules: Vec<DriftTimelineModule>,
    total_entries: usize,
}

pub(crate) async fn drift_timeline_handler(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<DriftTimelineQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_drift_timeline(shared.as_ref(), &params))
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

fn load_drift_timeline(
    shared: &SharedState,
    params: &DriftTimelineQuery,
) -> Result<DriftTimelineData, String> {
    let top = params.top.unwrap_or(10).clamp(1, 30);
    let since_days = super::common::parse_window_days(params.since.as_deref().or(Some("90d")));
    let cutoff = super::common::cutoff_millis_for_days(since_days);

    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(DriftTimelineData {
            modules: Vec::new(),
            total_entries: 0,
        });
    };

    let mut stmt = match conn.prepare(
        r#"
        SELECT symbol_name, drift_magnitude, file_path, detected_at
        FROM drift_results
        ORDER BY detected_at DESC
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => {
            return Ok(DriftTimelineData {
                modules: Vec::new(),
                total_entries: 0,
            });
        }
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut module_map: HashMap<String, Vec<DriftTimelinePoint>> = HashMap::new();
    let mut total = 0usize;

    for row in rows {
        let (symbol_name, magnitude, file_path, raw_detected) = row.map_err(|e| e.to_string())?;
        let ts = normalize_timestamp_millis(raw_detected);

        if let Some(c) = cutoff
            && ts < c
        {
            continue;
        }

        total += 1;
        let drift_score = magnitude.unwrap_or(0.0).clamp(0.0, 1.0);
        let module_name = std::path::Path::new(&file_path)
            .parent()
            .map(|pp| pp.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());

        module_map
            .entry(module_name)
            .or_default()
            .push(DriftTimelinePoint {
                timestamp: ts,
                drift_score,
                symbol_name: Some(symbol_name),
            });
    }

    // Sort each module's series by timestamp ascending
    for series in module_map.values_mut() {
        series.sort_by_key(|p| p.timestamp);
    }

    // Compute average score per module, take top N by avg score
    let mut modules: Vec<DriftTimelineModule> = module_map
        .into_iter()
        .map(|(name, series)| {
            let avg = if series.is_empty() {
                0.0
            } else {
                series.iter().map(|p| p.drift_score).sum::<f64>() / series.len() as f64
            };
            let point_count = series.len();
            DriftTimelineModule {
                name,
                avg_score: avg,
                point_count,
                series,
            }
        })
        .collect();

    modules.sort_by(|a, b| {
        b.avg_score
            .partial_cmp(&a.avg_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    modules.truncate(top);

    Ok(DriftTimelineData {
        modules,
        total_entries: total,
    })
}

fn normalize_timestamp_millis(raw: i64) -> i64 {
    if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw.max(0)
    }
}
