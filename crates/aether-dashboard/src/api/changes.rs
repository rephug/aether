use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::narrative::{classify_layer, layer_by_name};
use crate::support::{self, DashboardState};

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ChangesQuery {
    pub since: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChangesData {
    pub period: String,
    pub change_count: usize,
    pub changes: Vec<ChangeEntry>,
    pub file_summary: ChangeFileSummary,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChangeEntry {
    pub timestamp: String,
    #[serde(rename = "type")]
    pub change_type: String,
    pub file: String,
    pub layer: String,
    pub layer_icon: String,
    pub summary: String,
    pub symbols_affected: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_author: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChangeFileSummary {
    pub files_changed: usize,
    pub symbols_affected: usize,
    pub layers_touched: Vec<String>,
}

#[derive(Debug, Clone)]
struct PeriodSpec {
    label: &'static str,
    duration_secs: i64,
    git_since: &'static str,
}

#[derive(Debug, Clone)]
struct SymbolRow {
    id: String,
    symbol_name: String,
    qualified_name: String,
    file_path: String,
    last_seen_ms: i64,
    sir_version: i64,
    sir_updated_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct FileAccumulator {
    file_path: String,
    timestamp_ms: i64,
    file_exists: Option<bool>,
    file_mtime_ms: Option<i64>,
    sir_mtime_ms: Option<i64>,
    git_mtime_ms: Option<i64>,
    drift_mtime_ms: Option<i64>,
    git_message: Option<String>,
    git_author: Option<String>,
    git_status: Option<char>,
    sir_generated: bool,
    sir_updated: bool,
    symbols: Vec<String>,
    symbol_count: usize,
    representative_qualified_name: String,
    last_seen_ms: i64,
}

#[derive(Debug, Clone)]
struct GitChange {
    status: char,
    file_path: String,
    message: String,
    author: String,
    timestamp_ms: i64,
}

#[derive(Debug, Clone)]
struct TimedChange {
    timestamp_ms: i64,
    entry: ChangeEntry,
}

type SymbolsByFile = BTreeMap<String, Vec<SymbolRow>>;
type SymbolToFileMap = HashMap<String, String>;

pub(crate) async fn changes_handler(
    State(state): State<Arc<DashboardState>>,
    Query(query): Query<ChangesQuery>,
) -> impl IntoResponse {
    let shared = state.shared.clone();
    let since = query.since;
    let limit = query.limit;

    match support::run_blocking_with_timeout(move || {
        load_changes_data(shared.workspace.as_path(), since.as_deref(), limit)
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

pub(crate) fn load_changes_data(
    workspace: &Path,
    since: Option<&str>,
    limit: Option<usize>,
) -> Result<ChangesData, String> {
    let period = parse_period(since);
    let now_ms = support::current_unix_timestamp().saturating_mul(1000);
    let cutoff_ms = now_ms.saturating_sub(period.duration_secs.saturating_mul(1000));
    let limit = limit.unwrap_or(20).clamp(1, 100);

    let (symbols_by_file, symbol_to_file) = load_symbol_rows(workspace)?;
    let mut by_file = HashMap::<String, FileAccumulator>::new();

    for (file_path, symbols) in &symbols_by_file {
        let mut acc = FileAccumulator {
            file_path: file_path.clone(),
            ..FileAccumulator::default()
        };

        let mut names = symbols
            .iter()
            .map(|row| row.symbol_name.clone())
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        acc.symbol_count = names.len();
        acc.symbols = names;

        if let Some(row) = symbols.first() {
            acc.representative_qualified_name = row.qualified_name.clone();
        }

        for row in symbols {
            acc.last_seen_ms = acc.last_seen_ms.max(row.last_seen_ms);
            if row.sir_updated_ms >= cutoff_ms {
                acc.sir_mtime_ms = Some(acc.sir_mtime_ms.unwrap_or(0).max(row.sir_updated_ms));
                if row.sir_version <= 1 {
                    acc.sir_generated = true;
                } else {
                    acc.sir_updated = true;
                }
            }
        }

        by_file.insert(file_path.clone(), acc);
    }

    merge_file_mtime_signals(workspace, cutoff_ms, &mut by_file);
    merge_sir_mtime_signals(workspace, cutoff_ms, &symbol_to_file, &mut by_file);
    merge_git_signals(workspace, &period, cutoff_ms, &mut by_file);
    merge_drift_signals(workspace, cutoff_ms, &mut by_file)?;

    let mut changes = Vec::<TimedChange>::new();

    for mut acc in by_file.into_values() {
        if acc.timestamp_ms <= 0 {
            acc.timestamp_ms = acc
                .git_mtime_ms
                .or(acc.sir_mtime_ms)
                .or(acc.file_mtime_ms)
                .or(acc.drift_mtime_ms)
                .unwrap_or(acc.last_seen_ms);
        }

        if acc.timestamp_ms < cutoff_ms {
            continue;
        }

        let file_missing = matches!(acc.file_exists, Some(false));
        let change_type = classify_change_type(
            acc.git_status,
            file_missing,
            acc.sir_generated,
            acc.sir_updated,
        );
        let layer = classify_layer(
            acc.file_path.as_str(),
            acc.representative_qualified_name.as_str(),
            None,
        );
        let layer = layer_by_name(layer.name.as_str());

        let summary = summary_for_change(
            change_type,
            acc.file_path.as_str(),
            acc.symbol_count,
            layer.name.as_str(),
        );

        let timestamp = to_rfc3339(acc.timestamp_ms);

        changes.push(TimedChange {
            timestamp_ms: acc.timestamp_ms,
            entry: ChangeEntry {
                timestamp,
                change_type: change_type.to_owned(),
                file: acc.file_path,
                layer: layer.name,
                layer_icon: layer.icon,
                summary,
                symbols_affected: acc.symbols,
                git_message: acc.git_message,
                git_author: acc.git_author,
            },
        });
    }

    changes.sort_by(|left, right| {
        right
            .timestamp_ms
            .cmp(&left.timestamp_ms)
            .then_with(|| left.entry.file.cmp(&right.entry.file))
    });

    if changes.len() > limit {
        changes.truncate(limit);
    }

    let rendered = changes.into_iter().map(|row| row.entry).collect::<Vec<_>>();
    let summary = build_summary(rendered.as_slice());

    Ok(ChangesData {
        period: period.label.to_owned(),
        change_count: rendered.len(),
        changes: rendered,
        file_summary: summary,
    })
}

fn load_symbol_rows(workspace: &Path) -> Result<(SymbolsByFile, SymbolToFileMap), String> {
    let Some(conn) = support::open_meta_sqlite_ro(workspace).map_err(|err| err.to_string())? else {
        return Ok((BTreeMap::new(), HashMap::new()));
    };

    let sql_with_sir = r#"
        SELECT sy.id,
               sy.qualified_name,
               sy.file_path,
               sy.last_seen_at,
               COALESCE(sr.sir_version, 0),
               COALESCE(sr.updated_at, 0),
               COALESCE(sr.last_attempt_at, 0)
        FROM symbols sy
        LEFT JOIN sir sr ON sr.id = sy.id
        ORDER BY sy.file_path ASC, sy.qualified_name ASC, sy.id ASC
    "#;

    let mut stmt = conn.prepare(sql_with_sir).map_err(|err| {
        if support::is_missing_table(&err) {
            rusqlite::Error::QueryReturnedNoRows
        } else {
            err
        }
    });

    if matches!(stmt, Err(rusqlite::Error::QueryReturnedNoRows)) {
        stmt = conn.prepare(
            r#"
                SELECT id, qualified_name, file_path, last_seen_at,
                       0 AS sir_version,
                       0 AS updated_at,
                       0 AS last_attempt_at
                FROM symbols
                ORDER BY file_path ASC, qualified_name ASC, id ASC
            "#,
        );
    }

    let mut stmt = match stmt {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => {
            return Ok((BTreeMap::new(), HashMap::new()));
        }
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            let id = row.get::<_, String>(0)?;
            let qualified_name = row.get::<_, String>(1)?;
            let file_path = support::normalized_display_path(row.get::<_, String>(2)?.as_str());
            let last_seen_s = row.get::<_, i64>(3)?;
            let sir_version = row.get::<_, i64>(4)?;
            let updated_s = row.get::<_, i64>(5)?;
            let attempt_s = row.get::<_, i64>(6)?;

            Ok(SymbolRow {
                symbol_name: support::symbol_name_from_qualified(qualified_name.as_str()),
                id,
                qualified_name,
                file_path,
                last_seen_ms: last_seen_s.saturating_mul(1000),
                sir_version,
                sir_updated_ms: updated_s.max(attempt_s).saturating_mul(1000),
            })
        })
        .map_err(|err| err.to_string())?;

    let mut by_file = BTreeMap::<String, Vec<SymbolRow>>::new();
    let mut symbol_to_file = HashMap::<String, String>::new();

    for row in rows {
        let row = row.map_err(|err| err.to_string())?;
        symbol_to_file.insert(row.id.clone(), row.file_path.clone());
        by_file.entry(row.file_path.clone()).or_default().push(row);
    }

    Ok((by_file, symbol_to_file))
}

fn merge_file_mtime_signals(
    workspace: &Path,
    cutoff_ms: i64,
    by_file: &mut HashMap<String, FileAccumulator>,
) {
    for acc in by_file.values_mut() {
        let absolute = workspace.join(acc.file_path.as_str());
        match std::fs::metadata(&absolute) {
            Ok(meta) => {
                acc.file_exists = Some(true);
                if let Ok(modified) = meta.modified()
                    && let Some(ms) = system_time_millis(modified)
                    && ms >= cutoff_ms
                {
                    acc.file_mtime_ms = Some(acc.file_mtime_ms.unwrap_or(0).max(ms));
                    acc.timestamp_ms = acc.timestamp_ms.max(ms);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                acc.file_exists = Some(false);
                acc.timestamp_ms = acc.timestamp_ms.max(acc.last_seen_ms);
            }
            Err(_) => {}
        }
    }
}

fn merge_sir_mtime_signals(
    workspace: &Path,
    cutoff_ms: i64,
    symbol_to_file: &HashMap<String, String>,
    by_file: &mut HashMap<String, FileAccumulator>,
) {
    let sir_dir = workspace.join(".aether").join("sir");
    let Ok(entries) = std::fs::read_dir(sir_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        let Some(symbol_id) = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
        else {
            continue;
        };

        let Some(file_path) = symbol_to_file.get(symbol_id.as_str()) else {
            continue;
        };

        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Some(ms) = system_time_millis(modified) else {
            continue;
        };
        if ms < cutoff_ms {
            continue;
        }

        let acc = by_file
            .entry(file_path.clone())
            .or_insert_with(|| FileAccumulator {
                file_path: file_path.clone(),
                ..FileAccumulator::default()
            });
        acc.sir_mtime_ms = Some(acc.sir_mtime_ms.unwrap_or(0).max(ms));
        acc.timestamp_ms = acc.timestamp_ms.max(ms);
    }
}

fn merge_git_signals(
    workspace: &Path,
    period: &PeriodSpec,
    cutoff_ms: i64,
    by_file: &mut HashMap<String, FileAccumulator>,
) {
    if !is_git_workspace(workspace) {
        return;
    }

    let Ok(changes) = load_git_changes(workspace, period) else {
        return;
    };

    for change in changes {
        if change.timestamp_ms < cutoff_ms {
            continue;
        }

        let acc = by_file
            .entry(change.file_path.clone())
            .or_insert_with(|| FileAccumulator {
                file_path: change.file_path.clone(),
                ..FileAccumulator::default()
            });

        acc.git_mtime_ms = Some(acc.git_mtime_ms.unwrap_or(0).max(change.timestamp_ms));
        acc.timestamp_ms = acc.timestamp_ms.max(change.timestamp_ms);

        if acc.git_message.is_none() {
            acc.git_message = Some(change.message.clone());
        }
        if acc.git_author.is_none() {
            acc.git_author = Some(change.author.clone());
        }

        let existing = acc.git_status.unwrap_or('M');
        acc.git_status = Some(select_status(existing, change.status));
    }
}

fn merge_drift_signals(
    workspace: &Path,
    cutoff_ms: i64,
    by_file: &mut HashMap<String, FileAccumulator>,
) -> Result<(), String> {
    let Some(conn) = support::open_meta_sqlite_ro(workspace).map_err(|err| err.to_string())? else {
        return Ok(());
    };

    let mut stmt = match conn.prepare(
        r#"
        SELECT file_path, MAX(detected_at)
        FROM drift_results
        WHERE TRIM(COALESCE(file_path, '')) <> ''
        GROUP BY file_path
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if support::is_missing_table(&err) => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };

    let rows = stmt
        .query_map([], |row| {
            Ok((
                support::normalized_display_path(row.get::<_, String>(0)?.as_str()),
                row.get::<_, i64>(1)?,
            ))
        })
        .map_err(|err| err.to_string())?;

    for row in rows {
        let (file_path, raw_detected) = row.map_err(|err| err.to_string())?;
        let detected_ms = normalize_timestamp_millis(raw_detected);
        if detected_ms < cutoff_ms {
            continue;
        }

        let acc = by_file
            .entry(file_path.clone())
            .or_insert_with(|| FileAccumulator {
                file_path,
                ..FileAccumulator::default()
            });
        acc.drift_mtime_ms = Some(acc.drift_mtime_ms.unwrap_or(0).max(detected_ms));
        acc.timestamp_ms = acc.timestamp_ms.max(detected_ms);
    }

    Ok(())
}

fn summary_for_change(change_type: &str, file: &str, symbol_count: usize, layer: &str) -> String {
    let n = symbol_count;
    match change_type {
        "sir_generated" => {
            format!("AETHER analyzed {n} components in {file} for the first time")
        }
        "sir_updated" => {
            format!("AETHER's understanding of {n} components in {file} was refreshed")
        }
        "file_added" => {
            format!("New file {file} added to the {layer} layer with {n} components")
        }
        "file_deleted" => {
            format!("{file} removed - {n} components no longer tracked")
        }
        _ => {
            format!("{file} updated - affects {n} components in the {layer} layer")
        }
    }
}

fn classify_change_type(
    git_status: Option<char>,
    file_missing: bool,
    sir_generated: bool,
    sir_updated: bool,
) -> &'static str {
    if matches!(git_status, Some('D')) || file_missing {
        return "file_deleted";
    }
    if matches!(git_status, Some('A')) {
        return "file_added";
    }
    if sir_generated {
        return "sir_generated";
    }
    if sir_updated {
        return "sir_updated";
    }
    "file_modified"
}

fn build_summary(changes: &[ChangeEntry]) -> ChangeFileSummary {
    let mut symbols = BTreeSet::<String>::new();
    let mut layers = BTreeSet::<String>::new();

    for change in changes {
        layers.insert(change.layer.clone());
        for symbol in &change.symbols_affected {
            symbols.insert(symbol.clone());
        }
    }

    ChangeFileSummary {
        files_changed: changes.len(),
        symbols_affected: symbols.len(),
        layers_touched: layers.into_iter().collect(),
    }
}

fn is_git_workspace(workspace: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn load_git_changes(workspace: &Path, period: &PeriodSpec) -> Result<Vec<GitChange>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args([
            "log",
            period.git_since,
            "--format=%H|%s|%an|%aI",
            "--name-status",
        ])
        .output()
        .map_err(|err| err.to_string())?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut out = Vec::<GitChange>::new();
    let mut current_message = String::new();
    let mut current_author = String::new();
    let mut current_timestamp_ms = 0i64;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }

        if !line.contains('\t') {
            let mut parts = line.splitn(4, '|');
            let _hash = parts.next();
            let message = parts.next();
            let author = parts.next();
            let timestamp = parts.next();

            if let (Some(message), Some(author), Some(timestamp)) = (message, author, timestamp) {
                current_message = message.trim().to_owned();
                current_author = author.trim().to_owned();
                current_timestamp_ms = parse_rfc3339_millis(timestamp.trim()).unwrap_or(0);
            }
            continue;
        }

        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.is_empty() {
            continue;
        }

        let status_raw = fields[0].trim();
        let Some(status) = status_raw.chars().next() else {
            continue;
        };

        let file_path = if status_raw.starts_with('R') || status_raw.starts_with('C') {
            fields.get(2).copied().unwrap_or("")
        } else {
            fields.get(1).copied().unwrap_or("")
        };

        let file_path = support::normalized_display_path(file_path);
        if file_path.trim().is_empty() {
            continue;
        }

        out.push(GitChange {
            status,
            file_path,
            message: current_message.clone(),
            author: current_author.clone(),
            timestamp_ms: current_timestamp_ms,
        });
    }

    Ok(out)
}

fn select_status(existing: char, incoming: char) -> char {
    fn rank(value: char) -> i32 {
        match value {
            'D' => 4,
            'A' => 3,
            'R' => 2,
            'M' => 1,
            _ => 0,
        }
    }

    if rank(incoming) > rank(existing) {
        incoming
    } else {
        existing
    }
}

fn parse_period(since: Option<&str>) -> PeriodSpec {
    let raw = since.unwrap_or("24h").trim().to_ascii_lowercase();

    match raw.as_str() {
        "1h" => PeriodSpec {
            label: "1h",
            duration_secs: 60 * 60,
            git_since: "--since=1 hour ago",
        },
        "7d" => PeriodSpec {
            label: "7d",
            duration_secs: 7 * 24 * 60 * 60,
            git_since: "--since=7 days ago",
        },
        "30d" => PeriodSpec {
            label: "30d",
            duration_secs: 30 * 24 * 60 * 60,
            git_since: "--since=30 days ago",
        },
        _ => PeriodSpec {
            label: "24h",
            duration_secs: 24 * 60 * 60,
            git_since: "--since=24 hours ago",
        },
    }
}

fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis())
}

fn to_rfc3339(ms: i64) -> String {
    let Some(dt) = DateTime::from_timestamp_millis(ms) else {
        return Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    };
    dt.with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn normalize_timestamp_millis(raw: i64) -> i64 {
    if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw.max(0)
    }
}

fn system_time_millis(value: SystemTime) -> Option<i64> {
    let duration = value.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_millis() as i64)
}
