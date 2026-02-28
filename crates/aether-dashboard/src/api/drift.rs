use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const DEFAULT_THRESHOLD: f64 = 0.0;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DriftQuery {
    pub since: Option<u32>,
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DriftEntry {
    pub symbol_id: String,
    pub symbol_name: String,
    pub drift_type: String,
    pub drift_magnitude: f64,
    pub drift_score: f64,
    pub file_path: String,
    pub detected_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DriftData {
    pub analysis_available: bool,
    pub drift_entries: Vec<DriftEntry>,
    pub total_checked: i64,
    pub drifted_count: i64,
    pub threshold_used: f64,
}

pub(crate) async fn drift_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<DriftQuery>,
) -> impl IntoResponse {
    match load_drift_data(state.shared.as_ref(), &query) {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => support::json_internal_error(err),
    }
}

pub(crate) fn load_drift_data(
    shared: &SharedState,
    query: &DriftQuery,
) -> Result<DriftData, String> {
    let threshold = query.threshold.unwrap_or(DEFAULT_THRESHOLD).clamp(0.0, 1.0);
    let since_days = query.since.map(|days| days.clamp(1, 3650));
    let cutoff_millis = since_days.map(|days| {
        let now_ms = support::current_unix_timestamp().saturating_mul(1000);
        now_ms.saturating_sub((days as i64).saturating_mul(24 * 60 * 60 * 1000))
    });

    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(empty_drift_data(threshold, false, 0, 0));
    };

    let (mut total_checked, mut drifted_count, mut summary_available) = read_drift_summary(&conn)?;

    let mut stmt = match conn.prepare(
        r#"
        SELECT symbol_id, symbol_name, drift_type, drift_magnitude, file_path, detected_at, drift_summary
        FROM drift_results
        ORDER BY detected_at DESC, result_id ASC
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => {
            return Ok(empty_drift_data(threshold, summary_available, total_checked, drifted_count));
        }
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            let raw_detected = row.get::<_, i64>(5)?;
            let detected_at = normalize_timestamp_millis(raw_detected);
            let drift_magnitude = row.get::<_, Option<f64>>(3)?.unwrap_or(0.0).clamp(0.0, 1.0);
            let drift_summary = row.get::<_, Option<String>>(6)?.and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            });

            Ok(DriftEntry {
                symbol_id: row.get::<_, String>(0)?,
                symbol_name: row.get::<_, String>(1)?,
                drift_type: row.get::<_, String>(2)?,
                drift_magnitude,
                drift_score: drift_magnitude,
                file_path: support::normalized_display_path(row.get::<_, String>(4)?.as_str()),
                detected_at,
                drift_summary,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    for row in rows {
        let entry = row.map_err(|e| e.to_string())?;
        if entry.drift_magnitude < threshold {
            continue;
        }
        if let Some(cutoff) = cutoff_millis
            && entry.detected_at < cutoff
        {
            continue;
        }
        entries.push(entry);
    }

    entries.sort_by(|left, right| {
        right
            .drift_magnitude
            .partial_cmp(&left.drift_magnitude)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.detected_at.cmp(&left.detected_at))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });

    if total_checked <= 0 {
        total_checked = entries.len() as i64;
    }
    if drifted_count <= 0 {
        drifted_count = entries.len() as i64;
    }
    summary_available = summary_available || !entries.is_empty();

    Ok(DriftData {
        analysis_available: summary_available,
        drift_entries: entries,
        total_checked: total_checked.max(0),
        drifted_count: drifted_count.max(0),
        threshold_used: threshold,
    })
}

fn read_drift_summary(conn: &rusqlite::Connection) -> Result<(i64, i64, bool), String> {
    let mut stmt = match conn.prepare(
        "SELECT symbols_analyzed, drift_detected FROM drift_analysis_state ORDER BY id ASC LIMIT 1",
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => return Ok((0, 0, false)),
        Err(err) => return Err(err.to_string()),
    };

    let summary = stmt.query_row([], |row| {
        Ok((
            row.get::<_, i64>(0)?.max(0),
            row.get::<_, i64>(1)?.max(0),
            true,
        ))
    });

    match summary {
        Ok(value) => Ok(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok((0, 0, false)),
        Err(err) => Err(err.to_string()),
    }
}

fn normalize_timestamp_millis(raw: i64) -> i64 {
    if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw.max(0)
    }
}

fn empty_drift_data(
    threshold: f64,
    analysis_available: bool,
    total_checked: i64,
    drifted_count: i64,
) -> DriftData {
    DriftData {
        analysis_available,
        drift_entries: Vec::new(),
        total_checked: total_checked.max(0),
        drifted_count: drifted_count.max(0),
        threshold_used: threshold,
    }
}
