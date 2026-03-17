use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const MAX_MODULES: usize = 30;
const MAX_DAYS: usize = 90;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StalenessHeatmapQuery {
    pub since: Option<String>,
    #[allow(dead_code)]
    pub granularity: Option<String>,
}

#[derive(Debug, Serialize)]
struct StalenessHeatmapData {
    modules: Vec<String>,
    dates: Vec<String>,
    cells: Vec<Vec<f64>>,
}

pub(crate) async fn staleness_heatmap_handler(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<StalenessHeatmapQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || {
        load_staleness_heatmap(shared.as_ref(), &params)
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

fn load_staleness_heatmap(
    shared: &SharedState,
    params: &StalenessHeatmapQuery,
) -> Result<StalenessHeatmapData, String> {
    let since_days = super::common::parse_window_days(params.since.as_deref().or(Some("30d")));
    let cutoff = super::common::cutoff_millis_for_days(since_days);

    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(StalenessHeatmapData {
            modules: Vec::new(),
            dates: Vec::new(),
            cells: Vec::new(),
        });
    };

    // Try fingerprint_history first for real staleness data
    let has_fingerprint_data =
        match conn.prepare("SELECT COUNT(*) FROM sir_fingerprint_history LIMIT 1") {
            Ok(mut stmt) => stmt.query_row([], |row| row.get::<_, i64>(0)).unwrap_or(0) > 0,
            Err(_) => false,
        };

    if has_fingerprint_data {
        load_from_fingerprint_history(&conn, cutoff)
    } else {
        // Fallback: use drift_results grouped by time and module
        load_from_drift_results(&conn, cutoff)
    }
}

fn load_from_fingerprint_history(
    conn: &rusqlite::Connection,
    cutoff: Option<i64>,
) -> Result<StalenessHeatmapData, String> {
    // Query fingerprint changes: group by date and by module (parent dir)
    // We join to symbols to get file_path for module grouping
    let base_query = r#"
        SELECT s.file_path, fh.timestamp, fh.source_changed, fh.neighbor_changed
        FROM sir_fingerprint_history fh
        JOIN symbols s ON s.id = fh.symbol_id
        ORDER BY fh.timestamp ASC
    "#;

    let mut stmt = match conn.prepare(base_query) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => {
            return Ok(StalenessHeatmapData {
                modules: Vec::new(),
                dates: Vec::new(),
                cells: Vec::new(),
            });
        }
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    // Accumulate: module -> date_key -> (change_count, total_count)
    let mut module_date_data: HashMap<String, BTreeMap<String, (usize, usize)>> = HashMap::new();

    for row in rows {
        let (file_path, timestamp, source_changed, _neighbor_changed) =
            row.map_err(|e| e.to_string())?;

        let ts_millis = normalize_timestamp_millis(timestamp);
        if let Some(c) = cutoff
            && ts_millis < c
        {
            continue;
        }

        let module_name = std::path::Path::new(&file_path)
            .parent()
            .map(|pp| pp.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());

        let date_key = epoch_millis_to_date_string(ts_millis);
        let entry = module_date_data
            .entry(module_name)
            .or_default()
            .entry(date_key)
            .or_insert((0, 0));
        entry.1 += 1;
        if source_changed {
            entry.0 += 1;
        }
    }

    build_heatmap_from_date_data(module_date_data)
}

fn load_from_drift_results(
    conn: &rusqlite::Connection,
    cutoff: Option<i64>,
) -> Result<StalenessHeatmapData, String> {
    let mut stmt = match conn.prepare(
        r#"
        SELECT file_path, detected_at, COALESCE(drift_magnitude, 0.0)
        FROM drift_results
        ORDER BY detected_at ASC
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => {
            return Ok(StalenessHeatmapData {
                modules: Vec::new(),
                dates: Vec::new(),
                cells: Vec::new(),
            });
        }
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    // Accumulate: module -> date_key -> (magnitude_sum, count)
    let mut module_date_data: HashMap<String, BTreeMap<String, (f64, usize)>> = HashMap::new();

    for row in rows {
        let (file_path, raw_ts, magnitude) = row.map_err(|e| e.to_string())?;
        let ts_millis = normalize_timestamp_millis(raw_ts);

        if let Some(c) = cutoff
            && ts_millis < c
        {
            continue;
        }

        let module_name = std::path::Path::new(&file_path)
            .parent()
            .map(|pp| pp.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());

        let date_key = epoch_millis_to_date_string(ts_millis);
        let entry = module_date_data
            .entry(module_name)
            .or_default()
            .entry(date_key)
            .or_insert((0.0, 0));
        entry.0 += magnitude.clamp(0.0, 1.0);
        entry.1 += 1;
    }

    // Convert to change-count style: use average magnitude as the staleness score
    let converted: HashMap<String, BTreeMap<String, (usize, usize)>> = module_date_data
        .into_iter()
        .map(|(module, date_map)| {
            let converted_map = date_map
                .into_iter()
                .map(|(date, (mag_sum, count))| {
                    // Encode avg magnitude * 100 as "change count" over 100 "total"
                    let avg = if count > 0 {
                        (mag_sum / count as f64).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let scaled = (avg * 100.0).round() as usize;
                    (date, (scaled, 100))
                })
                .collect();
            (module, converted_map)
        })
        .collect();

    build_heatmap_from_date_data(converted)
}

fn build_heatmap_from_date_data(
    module_date_data: HashMap<String, BTreeMap<String, (usize, usize)>>,
) -> Result<StalenessHeatmapData, String> {
    if module_date_data.is_empty() {
        return Ok(StalenessHeatmapData {
            modules: Vec::new(),
            dates: Vec::new(),
            cells: Vec::new(),
        });
    }

    // Collect all unique dates across all modules
    let mut all_dates: Vec<String> = module_date_data
        .values()
        .flat_map(|date_map| date_map.keys().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    all_dates.sort();
    all_dates.truncate(MAX_DAYS);

    // Sort modules by total activity (most active first), truncate
    let mut module_activity: Vec<(String, usize)> = module_date_data
        .iter()
        .map(|(module, date_map)| {
            let total: usize = date_map.values().map(|(changes, _)| *changes).sum();
            (module.clone(), total)
        })
        .collect();
    module_activity.sort_by(|a, b| b.1.cmp(&a.1));
    module_activity.truncate(MAX_MODULES);

    let modules: Vec<String> = module_activity.into_iter().map(|(name, _)| name).collect();

    // Build cells matrix: [module_idx][date_idx] = staleness score (0.0 to 1.0)
    let cells: Vec<Vec<f64>> = modules
        .iter()
        .map(|module| {
            all_dates
                .iter()
                .map(|date| {
                    module_date_data
                        .get(module)
                        .and_then(|date_map| date_map.get(date))
                        .map(|(changes, total)| {
                            if *total == 0 {
                                0.0
                            } else {
                                (*changes as f64 / *total as f64).clamp(0.0, 1.0)
                            }
                        })
                        .unwrap_or(0.0)
                })
                .collect()
        })
        .collect();

    Ok(StalenessHeatmapData {
        modules,
        dates: all_dates,
        cells,
    })
}

fn normalize_timestamp_millis(raw: i64) -> i64 {
    if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw.max(0)
    }
}

fn epoch_millis_to_date_string(millis: i64) -> String {
    let secs = millis / 1000;
    let days_since_epoch = secs / 86400;
    // Simple date calculation without external crate dependencies
    // Using the algorithm from https://howardhinnant.github.io/date_algorithms.html
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}
