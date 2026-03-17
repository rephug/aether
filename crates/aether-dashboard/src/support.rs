use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aether_core::normalize_path;
use aether_store::{DriftStore, SirStateStore};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, response::Html};
use chrono::{SecondsFormat, Utc};
use maud::{Markup, html};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::state::SharedState;

pub(crate) const STALE_WARN_AFTER_MINUTES: u64 = 30;
pub(crate) const SIR_EXCERPT_MAX_LEN: usize = 160;
pub(crate) const GRAPH_QUERY_TIMEOUT_SECS: u64 = 10;
pub(crate) const GRAPH_QUERY_TIMEOUT_MESSAGE: &str = "This analysis is taking too long. Try reducing the graph scope or run `aetherd health` from the CLI for faster results.";
const TIMEOUT_ERROR_PREFIX: &str = "__aether_dashboard_timeout__:";

#[derive(Clone)]
pub(crate) struct DashboardState {
    pub shared: Arc<SharedState>,
    pub started_at: Instant,
}

impl DashboardState {
    pub fn new(shared: Arc<SharedState>) -> Self {
        Self {
            shared,
            started_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApiMeta {
    pub generated_at: String,
    pub stale: bool,
    pub index_age_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApiEnvelope<T> {
    pub data: T,
    pub meta: ApiMeta,
}

#[derive(Debug, Clone)]
pub(crate) struct StalenessInfo {
    pub stale: bool,
    pub index_age_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OverviewData {
    pub analysis_available: bool,
    pub total_symbols: i64,
    pub total_files: i64,
    pub sir_count: i64,
    pub sir_coverage_pct: f64,
    pub languages: BTreeMap<String, i64>,
    pub graph_backend: String,
    pub vector_status: String,
    pub drift_count: i64,
    pub coupling_count: i64,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

pub(crate) fn api_json<T: Serialize>(shared: &SharedState, data: T) -> Json<ApiEnvelope<T>> {
    Json(ApiEnvelope {
        data,
        meta: build_api_meta(shared),
    })
}

pub(crate) fn build_api_meta(shared: &SharedState) -> ApiMeta {
    let staleness = compute_staleness(shared.workspace.as_path());
    ApiMeta {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        stale: staleness.stale,
        index_age_seconds: staleness.index_age_seconds,
    }
}

pub(crate) fn compute_staleness(workspace: &Path) -> StalenessInfo {
    let now = current_unix_timestamp();
    let last_indexed_at = read_last_indexed_at(workspace).unwrap_or(None);
    let Some(last_indexed_at) = last_indexed_at else {
        return StalenessInfo {
            stale: false,
            index_age_seconds: None,
        };
    };

    let age = now.saturating_sub(last_indexed_at).max(0);
    let stale = (age as u64) >= STALE_WARN_AFTER_MINUTES.saturating_mul(60);
    StalenessInfo {
        stale,
        index_age_seconds: Some(age),
    }
}

pub(crate) fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(crate) fn meta_sqlite_path(workspace: &Path) -> PathBuf {
    workspace.join(".aether").join("meta.sqlite")
}

pub(crate) fn read_last_indexed_at(workspace: &Path) -> Result<Option<i64>, rusqlite::Error> {
    let Some(conn) = open_meta_sqlite_ro(workspace)? else {
        return Ok(None);
    };

    let latest_sir = conn.query_row("SELECT MAX(updated_at) FROM sir", [], |row| row.get(0));
    match latest_sir {
        Ok(Some(ts)) => return Ok(Some(ts)),
        Ok(None) => {}
        Err(err) if is_missing_table(&err) => {}
        Err(err) => return Err(err),
    }

    let latest_symbol = conn.query_row("SELECT MAX(last_seen_at) FROM symbols", [], |row| {
        row.get(0)
    });
    match latest_symbol {
        Ok(ts) => Ok(ts),
        Err(err) if is_missing_table(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

pub(crate) fn open_meta_sqlite_ro(workspace: &Path) -> Result<Option<Connection>, rusqlite::Error> {
    let sqlite_path = meta_sqlite_path(workspace);
    if !sqlite_path.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(Some(conn))
}

pub(crate) fn is_missing_table(err: &rusqlite::Error) -> bool {
    err.to_string().contains("no such table")
}

pub(crate) fn count_table_rows(
    conn: &Connection,
    table_name: &str,
) -> Result<i64, rusqlite::Error> {
    let sql = format!("SELECT COUNT(*) FROM {table_name}");
    match conn.query_row(&sql, [], |row| row.get::<_, i64>(0)) {
        Ok(value) => Ok(value.max(0)),
        Err(err) if is_missing_table(&err) => Ok(0),
        Err(err) => Err(err),
    }
}

pub(crate) fn count_distinct_files(conn: &Connection) -> Result<i64, rusqlite::Error> {
    match conn.query_row("SELECT COUNT(DISTINCT file_path) FROM symbols", [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(value) => Ok(value.max(0)),
        Err(err) if is_missing_table(&err) => Ok(0),
        Err(err) => Err(err),
    }
}

pub(crate) fn count_nonempty_sir(conn: &Connection) -> Result<i64, rusqlite::Error> {
    match conn.query_row(
        "SELECT COUNT(*) FROM sir WHERE TRIM(COALESCE(sir_json, '')) <> ''",
        [],
        |row| row.get::<_, i64>(0),
    ) {
        Ok(value) => Ok(value.max(0)),
        Err(err) if is_missing_table(&err) => Ok(0),
        Err(err) => Err(err),
    }
}

pub(crate) fn language_breakdown(
    conn: &Connection,
) -> Result<BTreeMap<String, i64>, rusqlite::Error> {
    let mut map = BTreeMap::new();
    let mut stmt = match conn.prepare(
        r#"
        SELECT COALESCE(NULLIF(TRIM(language), ''), 'unknown') AS language,
               COUNT(DISTINCT file_path) AS file_count
        FROM symbols
        GROUP BY language
        ORDER BY language ASC
        "#,
    ) {
        Ok(stmt) => stmt,
        Err(err) if is_missing_table(&err) => return Ok(map),
        Err(err) => return Err(err),
    };

    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?.max(0)))
    })?;

    for row in rows {
        let (lang, count) = row?;
        map.insert(lang, count);
    }
    Ok(map)
}

pub(crate) async fn load_overview_data(shared: &SharedState) -> Result<OverviewData, String> {
    let mut total_symbols = 0;
    let mut total_files = 0;
    let mut sir_count = 0;
    let mut languages = BTreeMap::new();

    match open_meta_sqlite_ro(shared.workspace.as_path()) {
        Ok(Some(conn)) => {
            total_symbols = count_table_rows(&conn, "symbols").map_err(|e| e.to_string())?;
            total_files = count_distinct_files(&conn).map_err(|e| e.to_string())?;
            sir_count = count_nonempty_sir(&conn).map_err(|e| e.to_string())?;
            languages = language_breakdown(&conn).map_err(|e| e.to_string())?;
        }
        Ok(None) => {}
        Err(err) => return Err(format!("failed to open dashboard sqlite: {err}")),
    }

    let sir_coverage_pct = if total_symbols > 0 {
        ((sir_count as f64) / (total_symbols as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let drift_count = shared
        .store
        .get_drift_analysis_state()
        .map_err(|e| e.to_string())?
        .map(|s| s.drift_detected.max(0))
        .unwrap_or(0);

    let coupling_count = if shared.config.storage.graph_backend.as_str() == "surreal" {
        load_surreal_coupling_count(shared)
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "dashboard: failed to count surreal co_change rows");
                0
            })
    } else {
        0
    };

    let vector_status = if !shared.config.embeddings.enabled {
        "disabled"
    } else if shared.vector_store.is_some() {
        "available"
    } else {
        "unavailable"
    }
    .to_owned();

    Ok(OverviewData {
        analysis_available: total_symbols > 0,
        total_symbols,
        total_files,
        sir_count,
        sir_coverage_pct,
        languages,
        graph_backend: shared.config.storage.graph_backend.as_str().to_owned(),
        vector_status,
        drift_count,
        coupling_count,
    })
}

async fn load_surreal_coupling_count(shared: &SharedState) -> Result<i64, String> {
    let graph = shared
        .surreal_graph_store()
        .await
        .map_err(|e| e.to_string())?;
    let mut response = graph
        .db()
        .query("SELECT VALUE count() FROM co_change GROUP ALL;")
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<Value> = response.take(0).map_err(|e| e.to_string())?;
    let count = rows.first().and_then(Value::as_i64).unwrap_or(0).max(0);
    Ok(count)
}

pub(crate) fn symbol_name_from_qualified(qualified_name: &str) -> String {
    qualified_name
        .rsplit("::")
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(qualified_name)
        .trim()
        .to_owned()
}

pub(crate) fn json_internal_error(message: impl Into<String>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody {
            error: "internal_error",
            message: message.into(),
        }),
    )
        .into_response()
}

pub(crate) fn json_timeout_error(message: impl Into<String>) -> Response {
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(ErrorBody {
            error: "timeout",
            message: message.into(),
        }),
    )
        .into_response()
}

pub(crate) fn timeout_error_message(message: impl Into<String>) -> String {
    format!("{TIMEOUT_ERROR_PREFIX}{}", message.into())
}

pub(crate) fn extract_timeout_error_message(message: &str) -> Option<String> {
    message
        .strip_prefix(TIMEOUT_ERROR_PREFIX)
        .map(ToOwned::to_owned)
}

pub(crate) async fn run_blocking_with_timeout<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let join = tokio::time::timeout(
        Duration::from_secs(GRAPH_QUERY_TIMEOUT_SECS),
        tokio::task::spawn_blocking(operation),
    )
    .await
    .map_err(|_| timeout_error_message(GRAPH_QUERY_TIMEOUT_MESSAGE))?;

    match join {
        Ok(result) => result,
        Err(err) => Err(format!("dashboard task join failure: {err}")),
    }
}

pub(crate) async fn run_async_with_timeout<T, F, Fut>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    tokio::time::timeout(Duration::from_secs(GRAPH_QUERY_TIMEOUT_SECS), operation())
        .await
        .map_err(|_| timeout_error_message(GRAPH_QUERY_TIMEOUT_MESSAGE))?
}

pub(crate) fn html_empty_state(title: &str, cmd: Option<&str>) -> Markup {
    html! {
        div class="empty-state" {
            // Empty state SVG icon — a folder with magnifying glass
            svg class="empty-state-icon" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" {
                rect x="4" y="12" width="32" height="24" rx="2" stroke="currentColor" stroke-width="1.5" fill="none" {}
                path d="M4 16h32" stroke="currentColor" stroke-width="1.5" {}
                path d="M4 12l4-4h10l4 4" stroke="currentColor" stroke-width="1.5" fill="none" {}
                circle cx="34" cy="30" r="8" stroke="currentColor" stroke-width="1.5" fill="none" {}
                line x1="40" y1="36" x2="46" y2="42" stroke="currentColor" stroke-width="2" stroke-linecap="round" {}
            }
            div class="empty-state-title" { (title) }
            @if let Some(cmd) = cmd {
                code class="empty-state-cmd" { (cmd) }
            }
        }
    }
}

pub(crate) fn html_error_state(title: &str, detail: &str) -> Markup {
    html! {
        div class="empty-state" {
            svg class="empty-state-icon" viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" {
                circle cx="24" cy="24" r="20" stroke="currentColor" stroke-width="1.5" fill="none" {}
                path d="M24 14v12" stroke="currentColor" stroke-width="2" stroke-linecap="round" {}
                circle cx="24" cy="32" r="1.5" fill="currentColor" {}
            }
            div class="empty-state-title" { (title) }
            div class="empty-state-msg" { (detail) }
        }
    }
}

pub(crate) fn html_markup_response(markup: Markup) -> Html<String> {
    Html(markup.into_string())
}

pub(crate) fn help_icon(tooltip: &str) -> Markup {
    html! {
        span class="metric-help" tabindex="0" data-tippy-content=(tooltip) aria-label="What does this mean?" { "ⓘ" }
    }
}

pub(crate) fn metric_label_with_tooltip(label: &str, tooltip: &str) -> Markup {
    html! {
        span class="inline-flex items-center gap-1" {
            span { (label) }
            (help_icon(tooltip))
        }
    }
}

pub(crate) fn explanation_header(
    title: &str,
    beginner_text: &str,
    intermediate_text: &str,
    expert_text: &str,
) -> Markup {
    html! {
        div class="explanation-header rounded-lg bg-slate-50 p-4 mb-6 border border-slate-200/80" {
            h2 class="text-base font-semibold text-slate-900" { (title) }
            p class="mt-2 text-sm text-slate-700 beginner-only" { (beginner_text) }
            p class="mt-2 text-sm text-slate-700 intermediate-only" { (intermediate_text) }
            p class="mt-2 text-sm text-slate-700 expert-only" { (expert_text) }
        }
    }
}

pub(crate) fn coupling_strength_label(score: f64) -> &'static str {
    crate::narrative::qualify_coupling(score)
}

pub(crate) fn format_age_seconds(age_seconds: Option<i64>) -> String {
    let Some(age_seconds) = age_seconds else {
        return "never".to_owned();
    };
    if age_seconds < 60 {
        return format!("{}s", age_seconds.max(0));
    }
    let minutes = age_seconds / 60;
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{}h", hours);
    }
    let days = hours / 24;
    format!("{}d", days)
}

pub(crate) fn badge_class_for_kind(kind: &str) -> &'static str {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("fn") || kind.contains("function") || kind.contains("method") {
        "badge-cyan"
    } else if kind.contains("struct") || kind.contains("class") {
        "badge-purple"
    } else if kind.contains("enum") {
        "badge-orange"
    } else if kind.contains("trait") || kind.contains("interface") {
        "badge-green"
    } else if kind.contains("module") {
        "badge-yellow"
    } else {
        "badge-muted"
    }
}

pub(crate) fn badge_class_for_language(language: &str) -> &'static str {
    match language.trim().to_ascii_lowercase().as_str() {
        "rust" => "badge-orange",
        "python" => "badge-green",
        "typescript" | "javascript" => "badge-cyan",
        "go" => "badge-cyan",
        "java" => "badge-red",
        _ => "badge-muted",
    }
}

pub(crate) fn sir_excerpt_for_symbol(shared: &SharedState, symbol_id: &str) -> Option<String> {
    let blob = shared.store.read_sir_blob(symbol_id).ok().flatten()?;
    let value: Value = serde_json::from_str(&blob).ok()?;
    let obj = value.as_object()?;

    for key in ["intent", "purpose", "summary", "description"] {
        if let Some(text) = obj.get(key).and_then(Value::as_str) {
            return Some(truncate_text(text, SIR_EXCERPT_MAX_LEN));
        }
    }

    let compact = serde_json::to_string(obj).ok()?;
    Some(truncate_text(&compact, SIR_EXCERPT_MAX_LEN))
}

pub(crate) fn truncate_text(input: &str, max_chars: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut out = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

pub(crate) fn normalized_display_path(path: &str) -> String {
    normalize_path(path)
}

pub(crate) fn embedded_bytes_response(bytes: Vec<u8>, mime: &str) -> Response {
    let mut response = bytes.into_response();
    if let Ok(value) = HeaderValue::from_str(mime) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

/// Percent-encode a string for safe use in URL path segments and query parameters.
pub(crate) fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}
