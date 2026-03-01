use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::api::common;
use crate::support::{self, DashboardState};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TimeMachineQuery {
    pub at: Option<String>,
    pub layers: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TimeMachineNode {
    pub id: String,
    pub qualified_name: String,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_score_at_time: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TimeMachineEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub inferred_at_time: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TimeMachineEvent {
    pub event_type: String,
    pub symbol_id: String,
    pub qualified_name: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TimeRange {
    pub earliest: Option<i64>,
    pub latest: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TimeMachineData {
    pub at: i64,
    pub layers: Vec<String>,
    pub not_computed_edge_timestamps: bool,
    pub not_computed_removed_symbols: bool,
    pub aggregated_to_files: bool,
    pub nodes: Vec<TimeMachineNode>,
    pub edges: Vec<TimeMachineEdge>,
    pub events: Vec<TimeMachineEvent>,
    pub time_range: TimeRange,
}

pub(crate) async fn time_machine_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<TimeMachineQuery>,
) -> impl IntoResponse {
    match load_time_machine_data(
        state.shared.as_ref(),
        query.at.as_deref(),
        query.layers.as_deref(),
    )
    .await
    {
        Ok(data) => support::api_json(state.shared.as_ref(), data).into_response(),
        Err(err) => support::json_internal_error(err),
    }
}

pub(crate) async fn load_time_machine_data(
    shared: &crate::state::SharedState,
    at: Option<&str>,
    layers: Option<&str>,
) -> Result<TimeMachineData, String> {
    let at_ms = parse_iso_to_millis(at).unwrap_or_else(|| support::current_unix_timestamp() * 1000);
    let layer_values = parse_layers(layers);

    let symbols = common::load_symbols(shared)?;
    let edges = common::load_dependency_algo_edges(shared)?;
    let communities = common::louvain_map(shared, &edges).await;

    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(TimeMachineData {
            at: at_ms,
            layers: layer_values,
            not_computed_edge_timestamps: true,
            not_computed_removed_symbols: true,
            aggregated_to_files: false,
            nodes: Vec::new(),
            edges: Vec::new(),
            events: Vec::new(),
            time_range: TimeRange {
                earliest: None,
                latest: None,
            },
        });
    };

    let (earliest, latest) = read_time_range(&conn);

    let mut existed = HashSet::<String>::new();
    {
        let mut stmt = match conn.prepare(
            r#"
            SELECT symbol_id
            FROM sir_history
            GROUP BY symbol_id
            HAVING MIN(created_at) <= ?1
            "#,
        ) {
            Ok(stmt) => stmt,
            Err(err) if support::is_missing_table(&err) => conn
                .prepare("SELECT id FROM symbols WHERE last_seen_at * 1000 <= ?1")
                .map_err(|e| e.to_string())?,
            Err(err) => return Err(err.to_string()),
        };

        let rows = stmt
            .query_map([at_ms], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            existed.insert(row.map_err(|e| e.to_string())?);
        }
    }

    let by_id = symbols
        .iter()
        .map(|row| (row.id.clone(), row))
        .collect::<HashMap<_, _>>();

    let drift_at_time = drift_scores_at_or_before(&conn, at_ms)?;

    let mut nodes = existed
        .iter()
        .filter_map(|id| by_id.get(id.as_str()))
        .map(|symbol| TimeMachineNode {
            id: symbol.id.clone(),
            qualified_name: symbol.qualified_name.clone(),
            file_path: support::normalized_display_path(symbol.file_path.as_str()),
            community_id: communities.get(symbol.id.as_str()).copied(),
            drift_score_at_time: drift_at_time.get(symbol.id.as_str()).copied(),
        })
        .collect::<Vec<_>>();

    nodes.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));

    let mut edge_rows = edges
        .iter()
        .filter(|edge| {
            existed.contains(edge.source_id.as_str()) && existed.contains(edge.target_id.as_str())
        })
        .map(|edge| TimeMachineEdge {
            source: edge.source_id.clone(),
            target: edge.target_id.clone(),
            edge_type: edge.edge_kind.clone(),
            inferred_at_time: true,
        })
        .collect::<Vec<_>>();
    edge_rows.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.edge_type.cmp(&right.edge_type))
    });

    let mut aggregated_to_files = false;
    if nodes.len() > 500 {
        aggregated_to_files = true;
        let (file_nodes, file_edges) = aggregate_to_files(nodes.as_slice(), edge_rows.as_slice());
        nodes = file_nodes;
        edge_rows = file_edges;
    }

    let events = events_around_timestamp(&conn, at_ms, by_id)?;

    Ok(TimeMachineData {
        at: at_ms,
        layers: layer_values,
        not_computed_edge_timestamps: true,
        not_computed_removed_symbols: true,
        aggregated_to_files,
        nodes,
        edges: edge_rows,
        events,
        time_range: TimeRange { earliest, latest },
    })
}

fn parse_layers(layers: Option<&str>) -> Vec<String> {
    let raw = layers.unwrap_or("deps,drift");
    let mut out = raw
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if out.is_empty() {
        out = vec!["deps".to_owned(), "drift".to_owned()];
    }
    out.sort();
    out.dedup();
    out
}

fn parse_iso_to_millis(input: Option<&str>) -> Option<i64> {
    let raw = input?.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&Utc).timestamp_millis())
}

fn read_time_range(conn: &rusqlite::Connection) -> (Option<i64>, Option<i64>) {
    let history_range = conn.query_row(
        "SELECT MIN(created_at), MAX(created_at) FROM sir_history",
        [],
        |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
    );

    match history_range {
        Ok((Some(min), Some(max))) => (Some(min), Some(max)),
        _ => {
            let symbol_range = conn.query_row(
                "SELECT MIN(last_seen_at * 1000), MAX(last_seen_at * 1000) FROM symbols",
                [],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            );
            symbol_range.unwrap_or((None, None))
        }
    }
}

fn drift_scores_at_or_before(
    conn: &rusqlite::Connection,
    at_ms: i64,
) -> Result<HashMap<String, f64>, String> {
    let mut stmt = match conn.prepare(
        r#"
        SELECT symbol_id, MAX(COALESCE(drift_magnitude, 0.0))
        FROM drift_results
        WHERE detected_at <= ?1
        GROUP BY symbol_id
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => return Ok(HashMap::new()),
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([at_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = HashMap::new();
    for row in rows {
        let (id, score) = row.map_err(|e| e.to_string())?;
        out.insert(id, score.clamp(0.0, 1.0));
    }
    Ok(out)
}

fn events_around_timestamp(
    conn: &rusqlite::Connection,
    at_ms: i64,
    symbol_lookup: HashMap<String, &common::SymbolInfo>,
) -> Result<Vec<TimeMachineEvent>, String> {
    let mut events = Vec::<TimeMachineEvent>::new();
    let start = at_ms.saturating_sub(24 * 60 * 60 * 1000);
    let end = at_ms.saturating_add(24 * 60 * 60 * 1000);

    if let Ok(mut stmt) = conn.prepare(
        r#"
        SELECT symbol_id, symbol_name, detected_at, drift_type
        FROM drift_results
        WHERE detected_at BETWEEN ?1 AND ?2
        ORDER BY detected_at ASC
        "#,
    ) {
        let rows = stmt
            .query_map([start, end], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (symbol_id, symbol_name, ts, drift_type) = row.map_err(|e| e.to_string())?;
            events.push(TimeMachineEvent {
                event_type: "drift".to_owned(),
                symbol_id,
                qualified_name: symbol_name,
                timestamp: ts,
                detail: Some(drift_type),
            });
        }
    }

    if let Ok(mut stmt) = conn.prepare(
        r#"
        SELECT symbol_id, MIN(created_at) AS first_at
        FROM sir_history
        GROUP BY symbol_id
        HAVING first_at BETWEEN ?1 AND ?2
        ORDER BY first_at ASC
        "#,
    ) {
        let rows = stmt
            .query_map([start, end], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (symbol_id, first_at) = row.map_err(|e| e.to_string())?;
            let qualified_name = symbol_lookup
                .get(symbol_id.as_str())
                .map(|row| row.qualified_name.clone())
                .unwrap_or_else(|| symbol_id.clone());
            events.push(TimeMachineEvent {
                event_type: "added".to_owned(),
                symbol_id,
                qualified_name,
                timestamp: first_at,
                detail: None,
            });
        }
    }

    events.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.event_type.cmp(&right.event_type))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });
    Ok(events)
}

fn aggregate_to_files(
    nodes: &[TimeMachineNode],
    edges: &[TimeMachineEdge],
) -> (Vec<TimeMachineNode>, Vec<TimeMachineEdge>) {
    let mut by_file = HashMap::<String, TimeMachineNode>::new();
    let mut symbol_to_file = HashMap::<String, String>::new();

    for node in nodes {
        symbol_to_file.insert(node.id.clone(), node.file_path.clone());
        by_file
            .entry(node.file_path.clone())
            .and_modify(|entry| {
                if entry.drift_score_at_time.unwrap_or(0.0)
                    < node.drift_score_at_time.unwrap_or(0.0)
                {
                    entry.drift_score_at_time = node.drift_score_at_time;
                }
            })
            .or_insert_with(|| TimeMachineNode {
                id: format!("file::{}", node.file_path),
                qualified_name: node.file_path.clone(),
                file_path: node.file_path.clone(),
                community_id: node.community_id,
                drift_score_at_time: node.drift_score_at_time,
            });
    }

    let mut edge_set = HashSet::<(String, String, String)>::new();
    for edge in edges {
        let Some(source_file) = symbol_to_file.get(edge.source.as_str()) else {
            continue;
        };
        let Some(target_file) = symbol_to_file.get(edge.target.as_str()) else {
            continue;
        };
        if source_file == target_file {
            continue;
        }
        edge_set.insert((
            format!("file::{source_file}"),
            format!("file::{target_file}"),
            edge.edge_type.clone(),
        ));
    }

    let mut file_nodes = by_file.into_values().collect::<Vec<_>>();
    file_nodes.sort_by(|left, right| left.file_path.cmp(&right.file_path));

    let mut file_edges = edge_set
        .into_iter()
        .map(|(source, target, edge_type)| TimeMachineEdge {
            source,
            target,
            edge_type,
            inferred_at_time: true,
        })
        .collect::<Vec<_>>();
    file_edges.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.edge_type.cmp(&right.edge_type))
    });

    (file_nodes, file_edges)
}
