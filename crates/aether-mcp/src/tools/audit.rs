use aether_core::normalize_path;
use aether_graph_algo::{
    GraphAlgorithmEdge, betweenness_centrality_sync, page_rank_sync,
    strongly_connected_components_sync,
};
use aether_store::{
    AuditFindingFilters, AuditSeverityCounts, AuditStore, DriftStore, NewAuditFinding,
    SirStateStore, SqliteStore, SymbolSearchResult,
};
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::AetherMcpServer;
use crate::AetherMcpError;

const DEFAULT_AUDIT_TOP_N: u32 = 20;
const MAX_AUDIT_TOP_N: u32 = 200;
const SQLITE_PARAM_CHUNK: usize = 900;
const RECENCY_WINDOW_DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const HIGH_BETWEENNESS_THRESHOLD: f64 = 0.1;
const LOW_CONFIDENCE_THRESHOLD: f64 = 0.7;
const HIGH_PAGERANK_PERCENTILE: f64 = 0.10;
const UNCERTAINTY_SCORE: f64 = 0.3;
const REASONING_HINT_WINDOW: usize = 200;
const UNCERTAINTY_TERMS: [&str; 9] = [
    "uncertain",
    "unsure",
    "cannot determine",
    "unclear",
    "might",
    "possibly",
    "latent",
    "cannot trace",
    "difficult to assess",
];

fn clamp_to_char_boundary_start(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn clamp_to_char_boundary_end(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditType {
    Symbol,
    CrossSymbol,
}

impl AuditType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::CrossSymbol => "cross_symbol",
        }
    }

    fn parse_db(value: &str) -> Result<Self, AetherMcpError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "symbol" => Ok(Self::Symbol),
            "cross_symbol" => Ok(Self::CrossSymbol),
            other => Err(AetherMcpError::Message(format!(
                "invalid audit_type stored in database: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Critical,
    High,
    Medium,
    Low,
    Informational,
}

impl AuditSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Informational => "informational",
        }
    }

    fn parse_db(value: &str) -> Result<Self, AetherMcpError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "critical" => Ok(Self::Critical),
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            "informational" => Ok(Self::Informational),
            other => Err(AetherMcpError::Message(format!(
                "invalid severity stored in database: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditCategory {
    Arithmetic,
    Encoding,
    SilentFailure,
    State,
    TypeSafety,
    Concurrency,
    ResourceLeak,
    LogicError,
}

impl AuditCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Arithmetic => "arithmetic",
            Self::Encoding => "encoding",
            Self::SilentFailure => "silent_failure",
            Self::State => "state",
            Self::TypeSafety => "type_safety",
            Self::Concurrency => "concurrency",
            Self::ResourceLeak => "resource_leak",
            Self::LogicError => "logic_error",
        }
    }

    fn parse_db(value: &str) -> Result<Self, AetherMcpError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "arithmetic" => Ok(Self::Arithmetic),
            "encoding" => Ok(Self::Encoding),
            "silent_failure" => Ok(Self::SilentFailure),
            "state" => Ok(Self::State),
            "type_safety" => Ok(Self::TypeSafety),
            "concurrency" => Ok(Self::Concurrency),
            "resource_leak" => Ok(Self::ResourceLeak),
            "logic_error" => Ok(Self::LogicError),
            other => Err(AetherMcpError::Message(format!(
                "invalid audit category stored in database: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditCertainty {
    Confirmed,
    Suspected,
    Theoretical,
}

impl AuditCertainty {
    fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Suspected => "suspected",
            Self::Theoretical => "theoretical",
        }
    }

    fn parse_db(value: &str) -> Result<Self, AetherMcpError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "confirmed" => Ok(Self::Confirmed),
            "suspected" => Ok(Self::Suspected),
            "theoretical" => Ok(Self::Theoretical),
            other => Err(AetherMcpError::Message(format!(
                "invalid certainty stored in database: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditStatus {
    Open,
    Confirmed,
    Fixed,
    Wontfix,
}

impl AuditStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Confirmed => "confirmed",
            Self::Fixed => "fixed",
            Self::Wontfix => "wontfix",
        }
    }

    fn parse_db(value: &str) -> Result<Self, AetherMcpError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "confirmed" => Ok(Self::Confirmed),
            "fixed" => Ok(Self::Fixed),
            "wontfix" => Ok(Self::Wontfix),
            other => Err(AetherMcpError::Message(format!(
                "invalid status stored in database: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditSubmitRequest {
    pub symbol_id: String,
    pub audit_type: Option<AuditType>,
    pub severity: AuditSeverity,
    pub category: AuditCategory,
    pub certainty: AuditCertainty,
    pub trigger_condition: String,
    pub impact: String,
    pub description: String,
    pub related_symbols: Option<Vec<String>>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditSubmitResponse {
    pub finding_id: i64,
    pub status: AuditStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditReportRequest {
    pub crate_filter: Option<String>,
    pub min_severity: Option<AuditSeverity>,
    pub category: Option<AuditCategory>,
    pub status: Option<AuditStatus>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditFindingOutput {
    pub id: i64,
    pub symbol_id: String,
    pub qualified_name: Option<String>,
    pub file_path: Option<String>,
    pub audit_type: AuditType,
    pub severity: AuditSeverity,
    pub category: AuditCategory,
    pub certainty: AuditCertainty,
    pub trigger_condition: String,
    pub impact: String,
    pub description: String,
    pub related_symbols: Vec<String>,
    pub model: String,
    pub status: AuditStatus,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditReportResponse {
    pub findings: Vec<AetherAuditFindingOutput>,
    pub summary: AuditSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditSummary {
    pub total: u32,
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub informational: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditResolveRequest {
    pub finding_id: i64,
    pub status: AuditStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditResolveResponse {
    pub finding_id: i64,
    pub new_status: AuditStatus,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditCandidatesRequest {
    /// Maximum number of candidates to return (default 20)
    pub top_n: Option<u32>,
    /// Scope to a specific crate (matches file_path prefix "crates/<name>/")
    pub crate_filter: Option<String>,
    /// Scope to a specific file path
    pub file_filter: Option<String>,
    /// Minimum structural risk score (0.0-1.0, default 0.0)
    pub min_risk: Option<f64>,
    /// Include reasoning_trace excerpts in output (default true)
    pub include_reasoning_hints: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditCandidate {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub kind: String,
    /// Composite risk score (0.0-1.0, higher = more risky)
    pub risk_score: f64,
    /// Human-readable risk factors
    pub risk_factors: Vec<String>,
    /// SIR confidence from current generation (if available)
    pub current_confidence: Option<f64>,
    /// Which pass generated the current SIR
    pub generation_pass: Option<String>,
    /// Excerpt from reasoning_trace highlighting uncertainty
    pub reasoning_hint: Option<String>,
    /// Composite priority score (0.0-1.0, higher = more urgent to audit)
    pub audit_priority: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAuditCandidatesResponse {
    pub candidates: Vec<AuditCandidate>,
    pub total_in_scope: u32,
    pub scope_description: String,
}

impl From<AuditSeverityCounts> for AuditSummary {
    fn from(value: AuditSeverityCounts) -> Self {
        Self {
            total: value.total,
            critical: value.critical,
            high: value.high,
            medium: value.medium,
            low: value.low,
            informational: value.informational,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct AuditSirMetadata {
    generation_pass: Option<String>,
    reasoning_trace: Option<String>,
    confidence: Option<f64>,
    has_sir: bool,
}

#[derive(Debug, Clone)]
struct AuditStructuralRow {
    symbol_id: String,
    qualified_name: String,
    file_path: String,
    kind: String,
    pagerank: f64,
    betweenness: f64,
    test_count: u32,
    risk_score: f64,
    in_cycle: bool,
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_required_text(value: &str, field_name: &str) -> Result<String, AetherMcpError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(AetherMcpError::Message(format!(
            "{field_name} must not be empty"
        )));
    }
    Ok(normalized.to_owned())
}

fn normalize_string_list(values: Option<Vec<String>>) -> Vec<String> {
    values
        .unwrap_or_default()
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect()
}

fn crate_filter_to_prefix(crate_filter: Option<&str>) -> Option<String> {
    let trimmed = crate_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    if trimmed.contains('/') {
        return Some(normalize_path(trimmed));
    }

    Some(format!("crates/{trimmed}/"))
}

fn normalize_optional_path(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_path)
}

fn normalize_generation_pass(value: Option<&str>) -> Option<String> {
    value.map(|value| {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            "scan".to_owned()
        } else {
            normalized
        }
    })
}

fn current_time_millis() -> i64 {
    super::current_unix_timestamp_millis()
}

fn recency_factor(last_accessed_at: Option<i64>, now_ms: i64) -> f64 {
    let Some(raw) = last_accessed_at else {
        return 1.0;
    };

    let timestamp_ms = if raw > 0 && raw < 1_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw
    };

    let age_ms = now_ms.saturating_sub(timestamp_ms).max(0);
    let max_window = RECENCY_WINDOW_DAYS.saturating_mul(DAY_MS) as f64;
    if max_window <= f64::EPSILON {
        return 0.0;
    }

    (age_ms as f64 / max_window).clamp(0.0, 1.0)
}

fn latest_semantic_drift_by_symbol(
    store: &SqliteStore,
) -> Result<HashMap<String, f64>, AetherMcpError> {
    let mut drift_by_symbol = HashMap::new();
    for record in store.list_drift_results(true)? {
        if record.drift_type != "semantic" {
            continue;
        }
        let Some(magnitude) = record.drift_magnitude else {
            continue;
        };
        drift_by_symbol
            .entry(record.symbol_id)
            .or_insert((magnitude as f64).clamp(0.0, 1.0));
    }
    Ok(drift_by_symbol)
}

fn parse_confidence(symbol_id: &str, sir_json: Option<&str>) -> Option<f64> {
    let sir_json = sir_json?.trim();
    if sir_json.is_empty() {
        return None;
    }

    match serde_json::from_str::<JsonValue>(sir_json) {
        Ok(value) => value
            .get("confidence")?
            .as_f64()
            .map(|value| value.clamp(0.0, 1.0)),
        Err(err) => {
            tracing::warn!(
                symbol_id,
                error = %err,
                "failed to parse SIR JSON while loading audit metadata"
            );
            None
        }
    }
}

fn load_sir_metadata_batch(
    conn: &Connection,
    symbol_ids: &[String],
) -> Result<HashMap<String, AuditSirMetadata>, AetherMcpError> {
    let normalized = symbol_ids
        .iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        return Ok(HashMap::new());
    }

    let mut metadata = HashMap::new();
    for chunk in normalized.chunks(SQLITE_PARAM_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            r#"
            SELECT id, generation_pass, reasoning_trace, sir_json, sir_status
            FROM sir
            WHERE id IN ({placeholders})
            ORDER BY id ASC
            "#
        );
        let params_vec = chunk
            .iter()
            .cloned()
            .map(SqlValue::Text)
            .collect::<Vec<_>>();
        let mut stmt = conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;

        for row in rows {
            let (symbol_id, generation_pass, reasoning_trace, sir_json, sir_status) = row?;
            let normalized_generation_pass = normalize_generation_pass(generation_pass.as_deref());
            let normalized_reasoning = reasoning_trace
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
            let sir_ready = sir_status
                .as_deref()
                .map(str::trim)
                .is_some_and(|status| status.eq_ignore_ascii_case("ready"));
            let has_blob = sir_json
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let confidence = parse_confidence(symbol_id.as_str(), sir_json.as_deref());

            metadata.insert(
                symbol_id,
                AuditSirMetadata {
                    generation_pass: normalized_generation_pass,
                    reasoning_trace: normalized_reasoning,
                    confidence,
                    has_sir: sir_ready || has_blob,
                },
            );
        }
    }

    Ok(metadata)
}

fn load_test_counts_by_symbol(conn: &Connection) -> Result<HashMap<String, u32>, AetherMcpError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT symbol_id, COUNT(*)
        FROM test_intents
        WHERE symbol_id IS NOT NULL
          AND COALESCE(TRIM(symbol_id), '') <> ''
        GROUP BY symbol_id
        ORDER BY symbol_id ASC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut counts = HashMap::new();
    for row in rows {
        let (symbol_id, count) = row?;
        counts.insert(symbol_id, count.max(0) as u32);
    }
    Ok(counts)
}

fn top_pagerank_symbol_ids(rows: &[AuditStructuralRow]) -> HashSet<String> {
    if rows.is_empty() {
        return HashSet::new();
    }

    let top_count = ((rows.len() as f64) * HIGH_PAGERANK_PERCENTILE).ceil() as usize;
    let top_count = top_count.max(1).min(rows.len());
    let mut ranked = rows
        .iter()
        .map(|row| (row.symbol_id.clone(), row.pagerank))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked
        .into_iter()
        .take(top_count)
        .map(|(symbol_id, _)| symbol_id)
        .collect()
}

fn find_uncertainty_match(text: &str) -> Option<(usize, usize)> {
    let lowered = text.to_ascii_lowercase();
    UNCERTAINTY_TERMS
        .iter()
        .filter_map(|term| lowered.find(term).map(|index| (index, term.len())))
        .min_by_key(|(index, _)| *index)
}

fn extract_reasoning_hint(text: &str) -> Option<String> {
    let (match_index, match_len) = find_uncertainty_match(text)?;
    let len = text.len();
    let start =
        clamp_to_char_boundary_start(text, match_index.saturating_sub(REASONING_HINT_WINDOW / 2));
    let end = clamp_to_char_boundary_end(
        text,
        (match_index + match_len + (REASONING_HINT_WINDOW / 2)).min(len),
    );
    let hint = text
        .get(start..end)?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if hint.is_empty() { None } else { Some(hint) }
}

fn reasoning_is_uncertain(text: Option<&str>) -> bool {
    text.is_some_and(|text| find_uncertainty_match(text).is_some())
}

fn pass_factor(generation_pass: Option<&str>) -> f64 {
    match normalize_generation_pass(generation_pass).as_deref() {
        Some("deep") => 0.0,
        Some("triage") => 0.1,
        _ => 0.3,
    }
}

fn build_audit_risk_factors(
    row: &AuditStructuralRow,
    generation_pass: Option<&str>,
    confidence: Option<f64>,
    reasoning_uncertain: bool,
    high_pagerank_ids: &HashSet<String>,
) -> Vec<String> {
    let mut factors = Vec::new();
    if row.betweenness > HIGH_BETWEENNESS_THRESHOLD {
        factors.push("high_betweenness".to_owned());
    }
    if row.in_cycle {
        factors.push("in_cycle".to_owned());
    }
    if row.test_count == 0 {
        factors.push("low_test_coverage".to_owned());
    }
    if high_pagerank_ids.contains(row.symbol_id.as_str()) {
        factors.push("high_pagerank".to_owned());
    }
    if normalize_generation_pass(generation_pass) != Some("deep".to_owned()) {
        factors.push("no_deep_analysis".to_owned());
    }
    if confidence.is_some_and(|value| value < LOW_CONFIDENCE_THRESHOLD) {
        factors.push("low_confidence".to_owned());
    }
    if reasoning_uncertain {
        factors.push("triage_uncertainty".to_owned());
    }
    factors
}

fn symbol_matches_scope(
    row: &AuditStructuralRow,
    crate_prefix: Option<&str>,
    file_filter: Option<&str>,
    min_risk: f64,
) -> bool {
    if row.risk_score < min_risk {
        return false;
    }
    if crate_prefix.is_some_and(|prefix| !row.file_path.starts_with(prefix)) {
        return false;
    }
    if file_filter.is_some_and(|file_path| row.file_path != file_path) {
        return false;
    }
    true
}

fn scope_description(
    crate_filter: Option<&str>,
    file_filter: Option<&str>,
    min_risk: f64,
) -> String {
    let mut parts = Vec::new();
    parts.push("scope=workspace".to_owned());
    if let Some(crate_filter) = crate_filter {
        parts.push(format!("crate_filter={crate_filter}"));
    }
    if let Some(file_filter) = file_filter {
        parts.push(format!("file_filter={file_filter}"));
    }
    parts.push(format!("min_risk={min_risk:.2}"));
    parts.join(", ")
}

impl AetherMcpServer {
    pub fn aether_audit_submit_logic(
        &self,
        request: AetherAuditSubmitRequest,
    ) -> Result<AetherAuditSubmitResponse, AetherMcpError> {
        self.state.require_writable()?;

        let symbol_id = normalize_required_text(&request.symbol_id, "symbol_id")?;
        let store = self.state.store.as_ref();

        if store.get_symbol_record(symbol_id.as_str())?.is_none() {
            return Err(AetherMcpError::Message(format!(
                "symbol_id not found in symbols table: {symbol_id}"
            )));
        }
        if store.get_sir_meta(symbol_id.as_str())?.is_none() {
            return Err(AetherMcpError::Message(format!(
                "symbol_id has no SIR row in store: {symbol_id}"
            )));
        }

        let finding_id = store.insert_audit_finding(NewAuditFinding {
            symbol_id,
            audit_type: request
                .audit_type
                .unwrap_or(AuditType::Symbol)
                .as_str()
                .to_owned(),
            severity: request.severity.as_str().to_owned(),
            category: request.category.as_str().to_owned(),
            certainty: request.certainty.as_str().to_owned(),
            trigger_condition: normalize_required_text(
                request.trigger_condition.as_str(),
                "trigger_condition",
            )?,
            impact: normalize_required_text(request.impact.as_str(), "impact")?,
            description: normalize_required_text(request.description.as_str(), "description")?,
            related_symbols: normalize_string_list(request.related_symbols),
            model: normalize_optional_text(request.model)
                .unwrap_or_else(|| "claude_code".to_owned()),
            provider: normalize_optional_text(request.provider)
                .unwrap_or_else(|| "manual".to_owned()),
            reasoning: normalize_optional_text(request.reasoning),
            status: AuditStatus::Open.as_str().to_owned(),
        })?;

        Ok(AetherAuditSubmitResponse {
            finding_id,
            status: AuditStatus::Open,
        })
    }

    pub fn aether_audit_report_logic(
        &self,
        request: AetherAuditReportRequest,
    ) -> Result<AetherAuditReportResponse, AetherMcpError> {
        let limit = request.limit.unwrap_or(50).clamp(1, 200);
        let min_severity = request.min_severity.unwrap_or(AuditSeverity::Low);
        let status = request.status.unwrap_or(AuditStatus::Open);
        let filters = AuditFindingFilters {
            symbol_id: None,
            file_path_prefix: crate_filter_to_prefix(request.crate_filter.as_deref()),
            min_severity: Some(min_severity.as_str().to_owned()),
            category: request.category.map(|value| value.as_str().to_owned()),
            status: Some(status.as_str().to_owned()),
            limit: Some(limit),
        };

        let store = self.state.store.as_ref();
        let findings = store.query_audit_findings(&filters)?;
        let summary = AuditSummary::from(store.count_audit_findings_by_severity(
            &AuditFindingFilters {
                limit: None,
                ..filters.clone()
            },
        )?);

        let symbol_ids = findings
            .iter()
            .map(|finding| finding.symbol_id.clone())
            .collect::<Vec<_>>();
        let symbol_records = store.get_symbol_search_results_batch(&symbol_ids)?;

        let findings = findings
            .into_iter()
            .map(|finding| {
                let symbol = symbol_records.get(&finding.symbol_id);
                Ok(AetherAuditFindingOutput {
                    id: finding.id,
                    symbol_id: finding.symbol_id,
                    qualified_name: symbol.map(|record| record.qualified_name.clone()),
                    file_path: symbol.map(|record| record.file_path.clone()),
                    audit_type: AuditType::parse_db(finding.audit_type.as_str())?,
                    severity: AuditSeverity::parse_db(finding.severity.as_str())?,
                    category: AuditCategory::parse_db(finding.category.as_str())?,
                    certainty: AuditCertainty::parse_db(finding.certainty.as_str())?,
                    trigger_condition: finding.trigger_condition,
                    impact: finding.impact,
                    description: finding.description,
                    related_symbols: finding.related_symbols,
                    model: finding.model,
                    status: AuditStatus::parse_db(finding.status.as_str())?,
                    created_at: finding.created_at,
                })
            })
            .collect::<Result<Vec<_>, AetherMcpError>>()?;

        Ok(AetherAuditReportResponse { findings, summary })
    }

    pub fn aether_audit_candidates_logic(
        &self,
        request: AetherAuditCandidatesRequest,
    ) -> Result<AetherAuditCandidatesResponse, AetherMcpError> {
        let top_n = request
            .top_n
            .unwrap_or(DEFAULT_AUDIT_TOP_N)
            .clamp(1, MAX_AUDIT_TOP_N);
        let crate_prefix = crate_filter_to_prefix(request.crate_filter.as_deref());
        let file_filter = normalize_optional_path(request.file_filter.as_deref());
        let min_risk = request.min_risk.unwrap_or(0.0).clamp(0.0, 1.0);
        let include_reasoning_hints = request.include_reasoning_hints.unwrap_or(true);
        let scope_description = scope_description(
            request
                .crate_filter
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty()),
            file_filter.as_deref(),
            min_risk,
        );

        let store = self.state.store.as_ref();
        let symbol_ids = store.list_all_symbol_ids()?;
        if symbol_ids.is_empty() {
            return Ok(AetherAuditCandidatesResponse {
                candidates: Vec::new(),
                total_in_scope: 0,
                scope_description,
            });
        }

        let symbol_records = store.get_symbol_search_results_batch(&symbol_ids)?;
        let conn = Connection::open(self.sqlite_path())?;
        let sir_metadata_by_symbol = load_sir_metadata_batch(&conn, &symbol_ids)?;
        let test_count_by_symbol = load_test_counts_by_symbol(&conn)?;
        let drift_by_symbol = latest_semantic_drift_by_symbol(store)?;
        let dependency_edges = store.list_graph_dependency_edges()?;
        let algo_edges = dependency_edges
            .iter()
            .map(|edge| GraphAlgorithmEdge {
                source_id: edge.source_symbol_id.clone(),
                target_id: edge.target_symbol_id.clone(),
                edge_kind: edge.edge_kind.clone(),
            })
            .collect::<Vec<_>>();
        let pagerank_scores = page_rank_sync(&algo_edges, 0.85, 25)
            .into_iter()
            .collect::<HashMap<_, _>>();
        let betweenness_scores = betweenness_centrality_sync(&algo_edges)
            .into_iter()
            .collect::<HashMap<_, _>>();
        let in_cycle_symbols = strongly_connected_components_sync(&algo_edges)
            .into_iter()
            .filter(|component| component.len() > 1)
            .flatten()
            .collect::<HashSet<_>>();

        let risk_weights = &self.state.config.health.risk_weights;
        let max_pagerank = pagerank_scores.values().copied().fold(0.0f64, f64::max);
        let now_ms = current_time_millis();
        let mut structural_rows = Vec::with_capacity(symbol_ids.len());

        for symbol_id in symbol_ids {
            let record = symbol_records
                .get(symbol_id.as_str())
                .cloned()
                .unwrap_or_else(|| SymbolSearchResult {
                    symbol_id: symbol_id.clone(),
                    qualified_name: symbol_id.clone(),
                    file_path: String::new(),
                    language: String::new(),
                    kind: String::new(),
                    access_count: 0,
                    last_accessed_at: None,
                });
            let file_path = normalize_path(record.file_path.as_str());
            let pagerank = pagerank_scores
                .get(symbol_id.as_str())
                .copied()
                .unwrap_or(0.0);
            let betweenness = betweenness_scores
                .get(symbol_id.as_str())
                .copied()
                .unwrap_or(0.0);
            let test_count = test_count_by_symbol
                .get(symbol_id.as_str())
                .copied()
                .unwrap_or(0);
            let test_coverage_ratio = ((test_count as f64) / 3.0).min(1.0);
            let drift_magnitude = drift_by_symbol
                .get(symbol_id.as_str())
                .copied()
                .unwrap_or(0.0);
            let pagerank_normalized = if max_pagerank > f64::EPSILON {
                pagerank / max_pagerank
            } else {
                0.0
            };
            let access_recency_factor = recency_factor(record.last_accessed_at, now_ms);
            let no_sir_factor = if sir_metadata_by_symbol
                .get(symbol_id.as_str())
                .is_some_and(|metadata| metadata.has_sir)
            {
                0.0
            } else {
                1.0
            };
            let risk_score = (risk_weights.pagerank * pagerank_normalized
                + risk_weights.test_gap * (1.0 - test_coverage_ratio)
                + risk_weights.drift * drift_magnitude
                + risk_weights.no_sir * no_sir_factor
                + risk_weights.recency * access_recency_factor)
                .clamp(0.0, 1.0);

            structural_rows.push(AuditStructuralRow {
                symbol_id: symbol_id.clone(),
                qualified_name: record.qualified_name,
                file_path,
                kind: record.kind,
                pagerank,
                betweenness,
                test_count,
                risk_score,
                in_cycle: in_cycle_symbols.contains(symbol_id.as_str()),
            });
        }

        let high_pagerank_ids = top_pagerank_symbol_ids(&structural_rows);
        let mut candidates = structural_rows
            .into_iter()
            .filter(|row| {
                symbol_matches_scope(
                    row,
                    crate_prefix.as_deref(),
                    file_filter.as_deref(),
                    min_risk,
                )
            })
            .map(|row| {
                let metadata = sir_metadata_by_symbol.get(row.symbol_id.as_str());
                let generation_pass =
                    metadata.and_then(|metadata| metadata.generation_pass.clone());
                let current_confidence = metadata.and_then(|metadata| metadata.confidence);
                let reasoning_trace =
                    metadata.and_then(|metadata| metadata.reasoning_trace.as_deref());
                let reasoning_uncertain = reasoning_is_uncertain(reasoning_trace);
                let reasoning_hint = if include_reasoning_hints {
                    reasoning_trace.and_then(extract_reasoning_hint)
                } else {
                    None
                };
                let audit_priority = (0.50 * row.risk_score
                    + 0.25
                        * current_confidence
                            .map(|confidence| 1.0 - confidence)
                            .unwrap_or(1.0)
                    + 0.15
                        * if reasoning_uncertain {
                            UNCERTAINTY_SCORE
                        } else {
                            0.0
                        }
                    + 0.10 * pass_factor(generation_pass.as_deref()))
                .clamp(0.0, 1.0);
                let risk_factors = build_audit_risk_factors(
                    &row,
                    generation_pass.as_deref(),
                    current_confidence,
                    reasoning_uncertain,
                    &high_pagerank_ids,
                );

                AuditCandidate {
                    symbol_id: row.symbol_id.clone(),
                    qualified_name: row.qualified_name,
                    file_path: row.file_path,
                    kind: row.kind,
                    risk_score: row.risk_score,
                    risk_factors,
                    current_confidence,
                    generation_pass,
                    reasoning_hint,
                    audit_priority,
                }
            })
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| {
            right
                .audit_priority
                .partial_cmp(&left.audit_priority)
                .unwrap_or(Ordering::Equal)
                .then_with(|| {
                    right
                        .risk_score
                        .partial_cmp(&left.risk_score)
                        .unwrap_or(Ordering::Equal)
                })
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        let total_in_scope = candidates.len().min(u32::MAX as usize) as u32;
        candidates.truncate(top_n as usize);

        Ok(AetherAuditCandidatesResponse {
            candidates,
            total_in_scope,
            scope_description,
        })
    }

    pub fn aether_audit_resolve_logic(
        &self,
        request: AetherAuditResolveRequest,
    ) -> Result<AetherAuditResolveResponse, AetherMcpError> {
        self.state.require_writable()?;
        if matches!(request.status, AuditStatus::Open) {
            return Err(AetherMcpError::Message(
                "audit resolve status must be fixed, wontfix, or confirmed".to_owned(),
            ));
        }

        let resolved = self
            .state
            .store
            .as_ref()
            .resolve_audit_finding(request.finding_id, request.status.as_str())?;

        Ok(AetherAuditResolveResponse {
            finding_id: request.finding_id,
            new_status: request.status,
            resolved,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_core::{EdgeKind, SymbolEdge};
    use aether_store::{
        AuditStore, DriftResultRecord, DriftStore, SirMetaRecord, SirStateStore,
        SymbolCatalogStore, SymbolRecord, SymbolRelationStore, TestIntentRecord, TestIntentStore,
    };
    use tempfile::tempdir;

    use super::{
        AetherAuditCandidatesRequest, AetherAuditReportRequest, AetherAuditResolveRequest,
        AetherAuditSubmitRequest, AuditCategory, AuditCertainty, AuditSeverity, AuditStatus,
        REASONING_HINT_WINDOW, extract_reasoning_hint,
    };
    use crate::AetherMcpServer;

    fn write_test_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn seed_symbol_with_sir(
        workspace: &Path,
        symbol_id: &str,
        qualified_name: &str,
        file_path: &str,
    ) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: symbol_id.to_owned(),
                file_path: file_path.to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: qualified_name.to_owned(),
                signature_fingerprint: format!("sig-{symbol_id}"),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
        store
            .write_sir_blob(
                symbol_id,
                r#"{
                    "intent":"seeded sir",
                    "inputs":[],
                    "outputs":[],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.7
                }"#,
            )
            .expect("write sir");
    }

    fn now_millis() -> i64 {
        1_700_000_000_000
    }

    fn symbol_record(symbol_id: &str, qualified_name: &str, file_path: &str) -> SymbolRecord {
        SymbolRecord {
            id: symbol_id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{symbol_id}"),
            last_seen_at: now_millis(),
        }
    }

    fn seed_symbol(workspace: &Path, record: SymbolRecord) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store.upsert_symbol(record).expect("upsert symbol");
    }

    fn seed_sir(
        workspace: &Path,
        symbol_id: &str,
        confidence: f64,
        generation_pass: &str,
        reasoning_trace: Option<&str>,
    ) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .write_sir_blob(
                symbol_id,
                format!(
                    r#"{{
                        "intent":"seeded sir for {symbol_id}",
                        "inputs":[],
                        "outputs":[],
                        "side_effects":[],
                        "dependencies":[],
                        "error_modes":[],
                        "confidence":{confidence}
                    }}"#
                )
                .as_str(),
            )
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id.to_owned(),
                sir_hash: format!("hash-{symbol_id}"),
                sir_version: 1,
                provider: "test".to_owned(),
                model: "test".to_owned(),
                generation_pass: generation_pass.to_owned(),
                reasoning_trace: reasoning_trace.map(str::to_owned),
                prompt_hash: None,
                staleness_score: None,
                updated_at: now_millis(),
                sir_status: "ready".to_owned(),
                last_error: None,
                last_attempt_at: now_millis(),
            })
            .expect("upsert sir meta");
    }

    fn seed_test_intents(workspace: &Path, file_path: &str, symbol_id: &str, count: u32) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        let intents = (0..count)
            .map(|index| TestIntentRecord {
                intent_id: format!("intent-{symbol_id}-{index}"),
                file_path: file_path.to_owned(),
                test_name: format!("test_{symbol_id}_{index}"),
                intent_text: format!("covers {symbol_id} #{index}"),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: Some(symbol_id.to_owned()),
                created_at: now_millis(),
                updated_at: now_millis(),
            })
            .collect::<Vec<_>>();
        store
            .replace_test_intents_for_file(file_path, intents.as_slice())
            .expect("replace test intents");
    }

    fn seed_drift(workspace: &Path, symbol_id: &str, file_path: &str, magnitude: f32) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .upsert_drift_results(&[DriftResultRecord {
                result_id: format!("drift-{symbol_id}"),
                symbol_id: symbol_id.to_owned(),
                file_path: file_path.to_owned(),
                symbol_name: symbol_id.to_owned(),
                drift_type: "semantic".to_owned(),
                drift_magnitude: Some(magnitude),
                current_sir_hash: None,
                baseline_sir_hash: None,
                commit_range_start: None,
                commit_range_end: None,
                drift_summary: None,
                detail_json: "{}".to_owned(),
                detected_at: now_millis(),
                is_acknowledged: false,
            }])
            .expect("seed drift");
    }

    fn seed_edge(workspace: &Path, source_id: &str, target_qualified_name: &str, file_path: &str) {
        let store = aether_store::SqliteStore::open(workspace).expect("open store");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: source_id.to_owned(),
                target_qualified_name: target_qualified_name.to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: file_path.to_owned(),
            }])
            .expect("upsert edge");
    }

    fn seed_audit_candidates_fixture(workspace: &Path) -> AetherMcpServer {
        write_test_config(workspace);

        seed_symbol(
            workspace,
            symbol_record("sym-a", "crate::alpha", "crates/aether-mcp/src/a.rs"),
        );
        seed_symbol(
            workspace,
            symbol_record("sym-b", "crate::bridge", "crates/aether-mcp/src/b.rs"),
        );
        seed_symbol(
            workspace,
            symbol_record(
                "sym-c",
                "crate::store_helper",
                "crates/aether-store/src/c.rs",
            ),
        );
        seed_symbol(
            workspace,
            symbol_record("sym-d", "crate::delta", "crates/aether-store/src/d.rs"),
        );

        seed_sir(
            workspace,
            "sym-a",
            0.95,
            "deep",
            Some("clear execution path"),
        );
        seed_sir(
            workspace,
            "sym-b",
            0.20,
            "scan",
            Some("uncertain branch; might skip validation under partial state"),
        );
        seed_sir(
            workspace,
            "sym-c",
            0.75,
            "triage",
            Some("baseline reasoning"),
        );
        seed_sir(
            workspace,
            "sym-d",
            0.90,
            "deep",
            Some("stable cleanup path"),
        );

        seed_test_intents(workspace, "tests/audit_alpha.rs", "sym-a", 2);
        seed_test_intents(workspace, "tests/audit_store.rs", "sym-c", 1);
        seed_test_intents(workspace, "tests/audit_delta.rs", "sym-d", 2);
        seed_drift(workspace, "sym-b", "crates/aether-mcp/src/b.rs", 0.8);

        seed_edge(
            workspace,
            "sym-a",
            "crate::bridge",
            "crates/aether-mcp/src/a.rs",
        );
        seed_edge(
            workspace,
            "sym-c",
            "crate::bridge",
            "crates/aether-store/src/c.rs",
        );
        seed_edge(
            workspace,
            "sym-b",
            "crate::delta",
            "crates/aether-mcp/src/b.rs",
        );

        AetherMcpServer::new(workspace, false).expect("server")
    }

    #[test]
    fn audit_submit_report_and_resolve_flow_works() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path());
        seed_symbol_with_sir(
            temp.path(),
            "sym-audit",
            "crate::audit::run",
            "crates/aether-store/src/lib.rs",
        );
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let submit = server
            .aether_audit_submit_logic(AetherAuditSubmitRequest {
                symbol_id: "sym-audit".to_owned(),
                audit_type: None,
                severity: AuditSeverity::High,
                category: AuditCategory::SilentFailure,
                certainty: AuditCertainty::Confirmed,
                trigger_condition: "missing transaction rollback".to_owned(),
                impact: "orphaned data remains".to_owned(),
                description: "rollback is skipped on partial failure".to_owned(),
                related_symbols: Some(vec!["sym-helper".to_owned()]),
                model: None,
                provider: None,
                reasoning: Some("confirmed via test reproduction".to_owned()),
            })
            .expect("submit finding");
        assert!(submit.finding_id > 0);
        assert_eq!(submit.status, AuditStatus::Open);

        let report = server
            .aether_audit_report_logic(AetherAuditReportRequest {
                crate_filter: Some("aether-store".to_owned()),
                min_severity: Some(AuditSeverity::Low),
                category: None,
                status: None,
                limit: Some(10),
            })
            .expect("report findings");
        assert_eq!(report.findings.len(), 1);
        assert_eq!(
            report.findings[0].qualified_name.as_deref(),
            Some("crate::audit::run")
        );
        assert_eq!(
            report.findings[0].file_path.as_deref(),
            Some("crates/aether-store/src/lib.rs")
        );
        assert_eq!(report.summary.total, 1);
        assert_eq!(report.summary.high, 1);

        let resolve = server
            .aether_audit_resolve_logic(AetherAuditResolveRequest {
                finding_id: submit.finding_id,
                status: AuditStatus::Fixed,
            })
            .expect("resolve finding");
        assert!(resolve.resolved);
        assert_eq!(resolve.new_status, AuditStatus::Fixed);

        let store = aether_store::SqliteStore::open(temp.path()).expect("reopen store");
        let findings = store
            .query_audit_findings(&aether_store::AuditFindingFilters {
                status: Some("fixed".to_owned()),
                ..aether_store::AuditFindingFilters::default()
            })
            .expect("query resolved findings");
        assert_eq!(findings.len(), 1);
        assert!(findings[0].resolved_at.is_some());
    }

    #[test]
    fn audit_candidates_are_sorted_by_priority_descending() {
        let temp = tempdir().expect("tempdir");
        let server = seed_audit_candidates_fixture(temp.path());

        let response = server
            .aether_audit_candidates_logic(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: None,
                file_filter: None,
                min_risk: Some(0.0),
                include_reasoning_hints: Some(true),
            })
            .expect("audit candidates");

        assert_eq!(response.total_in_scope, 4);
        assert_eq!(response.candidates[0].symbol_id, "sym-b");
        assert!(
            response
                .candidates
                .windows(2)
                .all(|window| window[0].audit_priority >= window[1].audit_priority)
        );
        assert!(
            response.candidates[0]
                .reasoning_hint
                .as_deref()
                .is_some_and(|hint| hint.contains("uncertain"))
        );
    }

    #[test]
    fn audit_candidates_respect_crate_filter() {
        let temp = tempdir().expect("tempdir");
        let server = seed_audit_candidates_fixture(temp.path());

        let response = server
            .aether_audit_candidates_logic(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: Some("aether-store".to_owned()),
                file_filter: None,
                min_risk: Some(0.0),
                include_reasoning_hints: Some(true),
            })
            .expect("audit candidates");

        assert_eq!(response.total_in_scope, 2);
        assert!(
            response
                .candidates
                .iter()
                .all(|candidate| candidate.file_path.starts_with("crates/aether-store/"))
        );
    }

    #[test]
    fn audit_candidates_apply_min_risk_filter() {
        let temp = tempdir().expect("tempdir");
        let server = seed_audit_candidates_fixture(temp.path());
        let baseline = server
            .aether_audit_candidates_logic(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: None,
                file_filter: None,
                min_risk: Some(0.0),
                include_reasoning_hints: Some(true),
            })
            .expect("baseline candidates");
        let threshold = (baseline.candidates[1].risk_score + 0.01).clamp(0.0, 1.0);

        let filtered = server
            .aether_audit_candidates_logic(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: None,
                file_filter: None,
                min_risk: Some(threshold),
                include_reasoning_hints: Some(true),
            })
            .expect("filtered candidates");

        assert!(!filtered.candidates.is_empty());
        assert!(
            filtered
                .candidates
                .iter()
                .all(|candidate| candidate.risk_score >= threshold)
        );
        assert!(filtered.total_in_scope < baseline.total_in_scope);
    }

    #[test]
    fn audit_candidates_extract_reasoning_hint_window() {
        let hint = extract_reasoning_hint(
            "The model is mostly confident, but uncertain about ownership transfer and cannot determine whether cleanup is always reached.",
        )
        .expect("hint");

        assert!(hint.contains("uncertain"));
        assert!(hint.len() <= REASONING_HINT_WINDOW);
    }

    #[test]
    fn audit_candidates_extract_reasoning_hint_handles_multibyte_prefixes() {
        let text = format!("{} uncertain path after multibyte prefix", "あ".repeat(40));

        let hint = extract_reasoning_hint(&text).expect("hint");

        assert!(hint.contains("uncertain"));
    }

    #[test]
    fn audit_candidates_return_empty_when_scope_matches_nothing() {
        let temp = tempdir().expect("tempdir");
        let server = seed_audit_candidates_fixture(temp.path());

        let response = server
            .aether_audit_candidates_logic(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: None,
                file_filter: Some("crates/missing/src/lib.rs".to_owned()),
                min_risk: Some(0.0),
                include_reasoning_hints: Some(true),
            })
            .expect("empty candidates");

        assert_eq!(response.total_in_scope, 0);
        assert!(response.candidates.is_empty());
    }
}
