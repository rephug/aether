use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use aether_store::ProjectNoteStore;

use crate::state::SharedState;
use crate::support::{self, DashboardState};

const MAX_EVENTS: usize = 200;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MemoryTimelineQuery {
    pub since: Option<String>,
    pub types: Option<String>,
}

#[derive(Debug, Serialize)]
struct TimelineEvent {
    timestamp: i64,
    event_type: String,
    title: String,
    detail: String,
    affected_count: usize,
}

#[derive(Debug, Serialize)]
struct MemoryTimelineData {
    events: Vec<TimelineEvent>,
    total: usize,
}

pub(crate) async fn memory_timeline_handler(
    State(state): State<Arc<DashboardState>>,
    Query(params): Query<MemoryTimelineQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    match support::run_blocking_with_timeout(move || load_memory_timeline(shared.as_ref(), &params))
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

fn load_memory_timeline(
    shared: &SharedState,
    params: &MemoryTimelineQuery,
) -> Result<MemoryTimelineData, String> {
    let since_days = super::common::parse_window_days(params.since.as_deref().or(Some("90d")));
    let cutoff = super::common::cutoff_millis_for_days(since_days);
    let type_filter = params
        .types
        .as_deref()
        .unwrap_or("all")
        .to_ascii_lowercase();

    let include_memory = type_filter == "all" || type_filter.contains("memory");
    let include_semantic = type_filter == "all" || type_filter.contains("semantic");
    let include_structural = type_filter == "all" || type_filter.contains("structural");

    let mut events = Vec::new();

    // Memory events from project notes
    if include_memory {
        collect_memory_events(shared, cutoff, &mut events);
    }

    // Semantic events from drift results with high magnitude
    if include_semantic {
        collect_semantic_drift_events(shared, cutoff, &mut events);
    }

    // Structural events from recently added/updated symbols
    if include_structural {
        collect_structural_events(shared, cutoff, &mut events);
    }

    // Sort all events by timestamp descending
    events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    events.truncate(MAX_EVENTS);

    let total = events.len();
    Ok(MemoryTimelineData { events, total })
}

fn collect_memory_events(
    shared: &SharedState,
    cutoff: Option<i64>,
    events: &mut Vec<TimelineEvent>,
) {
    let notes = match shared.store.list_project_notes(100, cutoff, false) {
        Ok(notes) => notes,
        Err(_) => return,
    };

    for note in notes {
        let timestamp_ms = if note.updated_at > 0 && note.updated_at < 1_000_000_000_000 {
            note.updated_at.saturating_mul(1000)
        } else {
            note.updated_at.max(0)
        };
        let title = format!("Project note: {}", note.source_type);
        let detail = truncate_content(&note.content, 120);
        let affected = note.symbol_refs.len() + note.file_refs.len();
        events.push(TimelineEvent {
            timestamp: timestamp_ms,
            event_type: "memory".to_owned(),
            title,
            detail,
            affected_count: affected,
        });
    }
}

fn collect_semantic_drift_events(
    shared: &SharedState,
    cutoff: Option<i64>,
    events: &mut Vec<TimelineEvent>,
) {
    let Some(conn) = support::open_meta_sqlite_ro(shared.workspace.as_path())
        .ok()
        .flatten()
    else {
        return;
    };

    let mut stmt = match conn.prepare(
        r#"
        SELECT symbol_name, drift_magnitude, file_path, detected_at, drift_summary
        FROM drift_results
        WHERE COALESCE(drift_magnitude, 0.0) >= 0.5
        ORDER BY detected_at DESC
        LIMIT 100
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(_) => return,
    };

    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<f64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return,
    };

    for row in rows {
        let (symbol_name, magnitude, file_path, raw_ts, summary) = match row {
            Ok(values) => values,
            Err(_) => continue,
        };

        let ts = normalize_timestamp_millis(raw_ts);
        if let Some(c) = cutoff
            && ts < c
        {
            continue;
        }

        let mag = magnitude.unwrap_or(0.0);
        let title = format!("Semantic drift detected: {symbol_name}");
        let detail = summary
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| truncate_content(s, 120))
            .unwrap_or_else(|| {
                format!(
                    "Drift magnitude {:.0}% in {}",
                    mag * 100.0,
                    support::normalized_display_path(&file_path)
                )
            });

        events.push(TimelineEvent {
            timestamp: ts,
            event_type: "semantic".to_owned(),
            title,
            detail,
            affected_count: 1,
        });
    }
}

fn collect_structural_events(
    shared: &SharedState,
    cutoff: Option<i64>,
    events: &mut Vec<TimelineEvent>,
) {
    let Some(conn) = support::open_meta_sqlite_ro(shared.workspace.as_path())
        .ok()
        .flatten()
    else {
        return;
    };

    // Group recently added symbols by file
    let query = if let Some(c) = cutoff {
        // cutoff is in millis, last_seen_at may be seconds
        let cutoff_secs = c / 1000;
        format!(
            r#"
            SELECT file_path, COUNT(*) as sym_count, MAX(last_seen_at) as latest
            FROM symbols
            WHERE last_seen_at >= {cutoff_secs}
            GROUP BY file_path
            ORDER BY latest DESC
            LIMIT 50
            "#
        )
    } else {
        r#"
        SELECT file_path, COUNT(*) as sym_count, MAX(last_seen_at) as latest
        FROM symbols
        GROUP BY file_path
        ORDER BY latest DESC
        LIMIT 50
        "#
        .to_owned()
    };

    let mut stmt = match conn.prepare(&query) {
        Ok(stmt) => stmt,
        Err(_) => return,
    };

    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => return,
    };

    for row in rows {
        let (file_path, sym_count, latest_ts) = match row {
            Ok(values) => values,
            Err(_) => continue,
        };

        let ts = normalize_timestamp_millis(latest_ts);
        let display_path = support::normalized_display_path(&file_path);
        let title = format!("Symbols indexed: {display_path}");
        let detail = format!("{sym_count} symbol(s) in {display_path}");

        events.push(TimelineEvent {
            timestamp: ts,
            event_type: "structural".to_owned(),
            title,
            detail,
            affected_count: sym_count.max(0) as usize,
        });
    }
}

fn normalize_timestamp_millis(raw: i64) -> i64 {
    if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw.max(0)
    }
}

fn truncate_content(input: &str, max_chars: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}
