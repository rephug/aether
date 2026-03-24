use aether_core::normalize_path;
use aether_store::{
    AuditFindingFilters, AuditSeverityCounts, AuditStore, NewAuditFinding, SirStateStore,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AetherMcpServer;
use crate::AetherMcpError;

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

    use aether_store::{AuditStore, SirStateStore, SymbolCatalogStore, SymbolRecord};
    use tempfile::tempdir;

    use super::{
        AetherAuditReportRequest, AetherAuditResolveRequest, AetherAuditSubmitRequest,
        AuditCategory, AuditCertainty, AuditSeverity, AuditStatus,
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
}
