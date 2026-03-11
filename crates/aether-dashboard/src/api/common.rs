use std::collections::{HashMap, HashSet};

use aether_analysis::{GraphAlgorithmEdge, connected_components, louvain_communities, page_rank};
use aether_config::GraphBackend;

use crate::state::SharedState;
use crate::support;

#[derive(Debug, Clone)]
pub(crate) struct SymbolInfo {
    pub id: String,
    pub qualified_name: String,
    pub file_path: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CoChangeSignals {
    pub temporal: f64,
    pub structural: f64,
    pub semantic: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct CoChangeEdgeInfo {
    pub fused_score: f64,
    pub coupling_type: String,
    pub signals: CoChangeSignals,
}

pub(crate) fn parse_window_days(window: Option<&str>) -> Option<i64> {
    let raw = window.unwrap_or("7d").trim().to_ascii_lowercase();
    match raw.as_str() {
        "7d" => Some(7),
        "30d" => Some(30),
        "90d" => Some(90),
        "all" => None,
        _ => Some(7),
    }
}

pub(crate) fn cutoff_millis_for_days(days: Option<i64>) -> Option<i64> {
    days.map(|d| {
        let now_ms = support::current_unix_timestamp().saturating_mul(1000);
        now_ms.saturating_sub(d.saturating_mul(24 * 60 * 60 * 1000))
    })
}

pub(crate) fn load_symbols(shared: &SharedState) -> Result<Vec<SymbolInfo>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };

    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, qualified_name, file_path
            FROM symbols
            ORDER BY qualified_name ASC, id ASC
            "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(SymbolInfo {
                id: row.get::<_, String>(0)?,
                qualified_name: row.get::<_, String>(1)?,
                file_path: row.get::<_, String>(2)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub(crate) fn load_dependency_algo_edges(
    shared: &SharedState,
) -> Result<Vec<GraphAlgorithmEdge>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };

    let mut stmt = conn
        .prepare(
            r#"
            SELECT e.source_id, t.id, e.edge_kind
            FROM symbol_edges e
            JOIN symbols t ON t.qualified_name = e.target_qualified_name
            WHERE e.edge_kind IN ('calls', 'depends_on', 'type_ref', 'implements')
            "#,
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map([], |row| {
            Ok(GraphAlgorithmEdge {
                source_id: row.get::<_, String>(0)?,
                target_id: row.get::<_, String>(1)?,
                edge_kind: row.get::<_, String>(2)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub(crate) async fn pagerank_map(
    shared: &SharedState,
    fallback_edges: &[GraphAlgorithmEdge],
) -> HashMap<String, f64> {
    if shared.config.storage.graph_backend == GraphBackend::Surreal
        && let Ok(graph) = shared.surreal_graph_store().await
        && let Ok(scores) = graph.list_pagerank().await
    {
        return scores
            .into_iter()
            .map(|(id, score)| (id, score as f64))
            .collect();
    }

    let edges = fallback_edges.to_vec();
    tokio::task::spawn_blocking(move || page_rank(&edges, 0.85, 25))
        .await
        .unwrap_or_default()
}

pub(crate) async fn louvain_map(
    shared: &SharedState,
    fallback_edges: &[GraphAlgorithmEdge],
) -> HashMap<String, i64> {
    if shared.config.storage.graph_backend == GraphBackend::Surreal
        && let Ok(graph) = shared.surreal_graph_store().await
        && let Ok(values) = graph.list_louvain_communities().await
    {
        return values.into_iter().collect();
    }

    let edges = fallback_edges.to_vec();
    tokio::task::spawn_blocking(move || {
        louvain_communities(&edges)
            .into_iter()
            .map(|(id, community)| (id, community as i64))
            .collect()
    })
    .await
    .unwrap_or_default()
}

pub(crate) async fn connected_components_vec(
    shared: &SharedState,
    fallback_edges: &[GraphAlgorithmEdge],
) -> Vec<Vec<String>> {
    if shared.config.storage.graph_backend == GraphBackend::Surreal
        && let Ok(graph) = shared.surreal_graph_store().await
        && let Ok(values) = graph.list_connected_components().await
    {
        return values;
    }

    let edges = fallback_edges.to_vec();
    tokio::task::spawn_blocking(move || connected_components(&edges))
        .await
        .unwrap_or_default()
}

pub(crate) fn latest_drift_score_by_symbol(
    shared: &SharedState,
) -> Result<HashMap<String, f64>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(HashMap::new());
    };

    let stmt = conn
        .prepare(
            r#"
            SELECT symbol_id,
                   MAX(detected_at) AS latest_detected,
                   MAX(COALESCE(drift_magnitude, 0.0)) AS max_magnitude
            FROM drift_results
            GROUP BY symbol_id
            "#,
        )
        .map_err(|e| {
            if support::is_missing_table(&e) {
                rusqlite::Error::QueryReturnedNoRows
            } else {
                e
            }
        });

    let Ok(mut stmt) = stmt else {
        return Ok(HashMap::new());
    };

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = HashMap::new();
    for row in rows {
        let (symbol_id, score) = row.map_err(|e| e.to_string())?;
        out.insert(symbol_id, score.clamp(0.0, 1.0));
    }
    Ok(out)
}

pub(crate) fn test_count_by_symbol(shared: &SharedState) -> Result<HashMap<String, i64>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(HashMap::new());
    };

    let mut stmt = match conn.prepare(
        r#"
        SELECT symbol_id, COUNT(*)
        FROM test_intents
        WHERE TRIM(COALESCE(symbol_id, '')) <> ''
        GROUP BY symbol_id
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => return Ok(HashMap::new()),
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?.max(0)))
        })
        .map_err(|e| e.to_string())?;

    let mut out = HashMap::new();
    for row in rows {
        let (symbol_id, count) = row.map_err(|e| e.to_string())?;
        out.insert(symbol_id, count.max(0));
    }
    Ok(out)
}

pub(crate) fn symbols_with_sir(shared: &SharedState) -> Result<HashSet<String>, String> {
    let Some(conn) =
        support::open_meta_sqlite_ro(shared.workspace.as_path()).map_err(|e| e.to_string())?
    else {
        return Ok(HashSet::new());
    };

    let mut stmt = match conn.prepare(
        r#"
        SELECT id
        FROM sir
        WHERE TRIM(COALESCE(sir_json, '')) <> ''
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => return Ok(HashSet::new()),
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;

    let mut out = HashSet::new();
    for row in rows {
        out.insert(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub(crate) fn risk_score(
    pagerank: f64,
    drift: f64,
    has_sir: bool,
    test_count: i64,
    max_pagerank: f64,
) -> f64 {
    let pagerank_norm = if max_pagerank > f64::EPSILON {
        (pagerank / max_pagerank).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let test_penalty = if test_count > 0 { 0.0 } else { 0.2 };
    let sir_penalty = if has_sir { 0.0 } else { 0.2 };

    (0.45 * pagerank_norm + 0.35 * drift.clamp(0.0, 1.0) + test_penalty + sir_penalty)
        .clamp(0.0, 1.0)
}

pub(crate) fn risk_grade(score: f64) -> &'static str {
    match score {
        s if s >= 0.93 => "A+",
        s if s >= 0.85 => "A",
        s if s >= 0.78 => "B+",
        s if s >= 0.70 => "B",
        s if s >= 0.60 => "C",
        s if s >= 0.50 => "D",
        _ => "F",
    }
}

pub(crate) fn first_sentence(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    for sep in ['.', '!', '?'] {
        if let Some(idx) = trimmed.find(sep) {
            return trimmed[..=idx].trim().to_owned();
        }
    }
    trimmed.to_owned()
}

pub(crate) async fn co_change_between_files(
    shared: &SharedState,
    file_a: &str,
    file_b: &str,
) -> Option<CoChangeEdgeInfo> {
    if shared.config.storage.graph_backend != GraphBackend::Surreal {
        return None;
    }
    let graph = shared.surreal_graph_store().await.ok()?;
    let pair = match graph
        .get_co_change_edge(file_a, file_b)
        .await
        .ok()
        .flatten()
    {
        Some(value) => value,
        None => graph
            .get_co_change_edge(file_b, file_a)
            .await
            .ok()
            .flatten()?,
    };

    Some(CoChangeEdgeInfo {
        fused_score: pair.fused_score as f64,
        coupling_type: pair.coupling_type,
        signals: CoChangeSignals {
            temporal: pair.git_coupling as f64,
            structural: pair.static_signal as f64,
            semantic: pair.semantic_signal as f64,
        },
    })
}

pub(crate) fn parse_lookback_to_analyzer_input(raw: &str) -> String {
    let trimmed = raw.trim().to_ascii_lowercase();
    match trimmed.as_str() {
        "7d" => "7 days".to_owned(),
        "30d" => "30 days".to_owned(),
        "90d" => "90 days".to_owned(),
        _ => trimmed,
    }
}

pub(crate) fn sparkline_placeholder(value: f64, points: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(points);
    for idx in 0..points {
        let noise = ((idx as f64 % 5.0) - 2.0) * 0.01;
        out.push((value + noise).clamp(0.0, 1.0));
    }
    out
}
