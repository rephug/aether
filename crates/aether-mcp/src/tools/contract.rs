use std::collections::BTreeMap;

use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{
    IntentContractRecord, IntentViolationRecord, SirStateStore, SqliteStore, SymbolCatalogStore,
    SymbolRecord,
};
use aetherd::contracts::{ClauseStatus, ContractVerifier};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AetherMcpServer;
use crate::AetherMcpError;

const DEFAULT_VIOLATION_LIMIT: u32 = 20;
const MAX_VIOLATION_LIMIT: u32 = 1000;
const CLAUDE_CODE_CREATOR: &str = "claude_code";
const NOTE_ACTIVE_ONLY: &str =
    "include_inactive is not yet supported - showing active contracts only";
const NOTE_INVALID_CLAUSE_EMBEDDING: &str =
    "Stored clause embedding is invalid and could not be compared.";
const NOTE_NO_EMBEDDING: &str = "No embedding available for comparison.";
const NOTE_NO_SIR: &str = "No SIR available for comparison.";
const NOTE_INCOMPATIBLE_EMBEDDING: &str = "Stored embeddings are incompatible for comparison.";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractAddRequest {
    /// Symbol ID or qualified name
    pub symbol: String,
    /// Clause type: "must", "must_not", or "preserves"
    pub clause_type: String,
    /// Clause text describing the behavioral expectation
    pub clause_text: String,
    /// Who is creating this contract (default "claude_code")
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractAddResponse {
    pub contract_id: i64,
    pub symbol_id: String,
    pub clause_type: String,
    pub clause_text: String,
    pub has_embedding: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractListRequest {
    /// Optional symbol ID or qualified name to filter by
    pub symbol: Option<String>,
    /// Include inactive/deactivated contracts (default false)
    pub include_inactive: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContractOutput {
    pub id: i64,
    pub symbol_id: String,
    pub qualified_name: Option<String>,
    pub clause_type: String,
    pub clause_text: String,
    pub active: bool,
    pub violation_streak: i64,
    pub created_at: i64,
    pub created_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractListResponse {
    pub contracts: Vec<ContractOutput>,
    pub total: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractRemoveRequest {
    /// Contract ID to deactivate
    pub contract_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractRemoveResponse {
    pub contract_id: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractCheckRequest {
    /// Optional symbol ID or qualified name. If omitted, checks all contracted symbols.
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClauseResultOutput {
    pub contract_id: i64,
    pub clause_type: String,
    pub clause_text: String,
    pub status: String,
    pub similarity: Option<f64>,
    pub judge_reason: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SymbolCheckOutput {
    pub symbol_id: String,
    pub qualified_name: Option<String>,
    pub clauses_checked: u32,
    pub passed: u32,
    pub failed: u32,
    pub ambiguous: u32,
    pub clause_results: Vec<ClauseResultOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractCheckResponse {
    pub symbols_checked: Vec<SymbolCheckOutput>,
    pub summary: ContractCheckSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ContractCheckSummary {
    pub symbols_checked: u32,
    pub total_clauses: u32,
    pub passed: u32,
    pub failed: u32,
    pub ambiguous: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractViolationsRequest {
    /// Filter by symbol ID or qualified name (optional)
    pub symbol: Option<String>,
    /// Filter by contract ID (optional)
    pub contract_id: Option<i64>,
    /// Maximum violations to return (default 20)
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ViolationOutput {
    pub id: i64,
    pub contract_id: i64,
    pub symbol_id: String,
    pub qualified_name: Option<String>,
    pub sir_version: i64,
    pub violation_type: String,
    pub confidence: Option<f64>,
    pub reason: Option<String>,
    pub detected_at: i64,
    pub dismissed: bool,
    pub dismissed_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractViolationsResponse {
    pub violations: Vec<ViolationOutput>,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractDismissRequest {
    /// Violation ID to dismiss
    pub violation_id: i64,
    /// Reason for dismissal
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherContractDismissResponse {
    pub violation_id: i64,
    pub status: String,
}

fn normalize_optional_text(value: Option<String>, default: &str) -> String {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn validate_clause_type(value: &str) -> Result<String, AetherMcpError> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "must" | "must_not" | "preserves" => Ok(normalized),
        _ => Err(AetherMcpError::Message(format!(
            "clause_type must be one of: must, must_not, preserves (got '{}')",
            value.trim()
        ))),
    }
}

fn resolve_symbol_selector(
    store: &SqliteStore,
    selector: &str,
) -> Result<SymbolRecord, AetherMcpError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(AetherMcpError::Message(
            "symbol selector must not be empty".to_owned(),
        ));
    }

    if let Some(record) = store.get_symbol_record(selector)? {
        return Ok(record);
    }

    let exact_matches = store.find_symbol_search_results_by_qualified_name(selector)?;
    match exact_matches.as_slice() {
        [only] => {
            return store
                .get_symbol_record(only.symbol_id.as_str())?
                .ok_or_else(|| {
                    AetherMcpError::Message(format!(
                        "symbol search returned missing record: {}",
                        only.symbol_id
                    ))
                });
        }
        [] => {}
        many => {
            let candidates = many
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            return Err(AetherMcpError::Message(format!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            )));
        }
    }

    let matches = store.search_symbols(selector, 10)?;
    match matches.as_slice() {
        [] => Err(AetherMcpError::Message(format!(
            "symbol not found: {selector}"
        ))),
        [only] => store
            .get_symbol_record(only.symbol_id.as_str())?
            .ok_or_else(|| {
                AetherMcpError::Message(format!(
                    "symbol search returned missing record: {}",
                    only.symbol_id
                ))
            }),
        many => {
            let candidates = many
                .iter()
                .map(|candidate| {
                    format!(
                        "{} [{}]",
                        candidate.qualified_name.trim(),
                        candidate.file_path.trim()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n  - ");
            Err(AetherMcpError::Message(format!(
                "ambiguous symbol selector '{selector}'. Candidates:\n  - {candidates}"
            )))
        }
    }
}

fn qualified_names_by_symbol_id(
    store: &SqliteStore,
    symbol_ids: &[String],
) -> Result<BTreeMap<String, String>, AetherMcpError> {
    let records = store.get_symbol_search_results_batch(symbol_ids)?;
    Ok(records
        .into_iter()
        .map(|(symbol_id, record)| (symbol_id, record.qualified_name))
        .collect())
}

fn load_symbol_embedding(
    store: &SqliteStore,
    config: &aether_config::AetherConfig,
    symbol_id: &str,
) -> Result<Option<Vec<f32>>, AetherMcpError> {
    let provider = config.embeddings.provider.as_str();
    let model = config
        .embeddings
        .model
        .as_deref()
        .unwrap_or("gemini-embedding-2-preview");
    let records = store.list_symbol_embeddings_for_ids(provider, model, &[symbol_id.to_owned()])?;
    Ok(records.into_iter().next().map(|record| record.embedding))
}

fn clause_status_label(status: &ClauseStatus) -> &'static str {
    match status {
        ClauseStatus::Pass => "pass",
        ClauseStatus::Fail => "fail",
        ClauseStatus::Ambiguous => "ambiguous",
    }
}

fn embeddings_comparable(left: &[f32], right: &[f32]) -> bool {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return false;
    }

    let left_norm_sq = left
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>();
    let right_norm_sq = right
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>();

    left_norm_sq > f64::EPSILON && right_norm_sq > f64::EPSILON
}

fn violation_exists(
    sqlite_path: &std::path::Path,
    violation_id: i64,
) -> Result<bool, AetherMcpError> {
    let conn = Connection::open_with_flags(sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM intent_violations WHERE id = ?1 LIMIT 1",
            params![violation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn build_contract_output(
    contract: IntentContractRecord,
    qualified_name: Option<String>,
) -> ContractOutput {
    ContractOutput {
        id: contract.id,
        symbol_id: contract.symbol_id,
        qualified_name,
        clause_type: contract.clause_type,
        clause_text: contract.clause_text,
        active: contract.active,
        violation_streak: contract.violation_streak,
        created_at: contract.created_at,
        created_by: contract.created_by,
    }
}

fn build_violation_output(
    violation: IntentViolationRecord,
    qualified_name: Option<String>,
) -> ViolationOutput {
    ViolationOutput {
        id: violation.id,
        contract_id: violation.contract_id,
        symbol_id: violation.symbol_id,
        qualified_name,
        sir_version: violation.sir_version,
        violation_type: violation.violation_type,
        confidence: violation.confidence,
        reason: violation.reason,
        detected_at: violation.detected_at,
        dismissed: violation.dismissed,
        dismissed_reason: violation.dismissed_reason,
    }
}

impl AetherMcpServer {
    pub async fn maybe_embed_contract_clause(&self, clause_text: &str) -> Option<String> {
        let trimmed = clause_text.trim();
        if trimmed.is_empty() {
            return None;
        }

        match load_embedding_provider_from_config(
            self.workspace(),
            EmbeddingProviderOverrides::default(),
        ) {
            Ok(Some(loaded)) => match loaded.provider.embed_text(trimmed).await {
                Ok(embedding) if !embedding.is_empty() => match serde_json::to_string(&embedding) {
                    Ok(json) => Some(json),
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "failed to serialize contract clause embedding"
                        );
                        None
                    }
                },
                Ok(_) => {
                    tracing::warn!("embedding provider returned empty vector for contract clause");
                    None
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "embedding provider error while embedding contract clause"
                    );
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "failed to load embedding provider for contract clause"
                );
                None
            }
        }
    }

    pub fn aether_contract_add_logic(
        &self,
        request: AetherContractAddRequest,
        embedding_json: Option<String>,
    ) -> Result<AetherContractAddResponse, AetherMcpError> {
        self.state.require_writable()?;

        let store = self.state.store.as_ref();
        let symbol = resolve_symbol_selector(store, request.symbol.as_str())?;
        let clause_type = validate_clause_type(request.clause_type.as_str())?;
        let clause_text = request.clause_text.trim();
        if clause_text.is_empty() {
            return Err(AetherMcpError::Message(
                "clause_text must not be empty".to_owned(),
            ));
        }
        let created_by = normalize_optional_text(request.created_by, CLAUDE_CODE_CREATOR);

        let contract_id = store.insert_intent_contract(
            symbol.id.as_str(),
            clause_type.as_str(),
            clause_text,
            embedding_json.as_deref(),
            created_by.as_str(),
        )?;

        Ok(AetherContractAddResponse {
            contract_id,
            symbol_id: symbol.id,
            clause_type,
            clause_text: clause_text.to_owned(),
            has_embedding: embedding_json.is_some(),
            status: "created".to_owned(),
        })
    }

    pub fn aether_contract_list_logic(
        &self,
        request: AetherContractListRequest,
    ) -> Result<AetherContractListResponse, AetherMcpError> {
        let store = self.state.store.as_ref();
        let contracts = if let Some(symbol) = request.symbol.as_deref() {
            let symbol = resolve_symbol_selector(store, symbol)?;
            store.list_active_contracts_for_symbol(symbol.id.as_str())?
        } else {
            store.list_all_active_contracts()?
        };

        let symbol_ids = contracts
            .iter()
            .map(|contract| contract.symbol_id.clone())
            .collect::<Vec<_>>();
        let qualified_names = qualified_names_by_symbol_id(store, &symbol_ids)?;

        let outputs = contracts
            .into_iter()
            .map(|contract| {
                let qualified_name = qualified_names.get(contract.symbol_id.as_str()).cloned();
                build_contract_output(contract, qualified_name)
            })
            .collect::<Vec<_>>();

        Ok(AetherContractListResponse {
            total: outputs.len() as u32,
            contracts: outputs,
            note: request
                .include_inactive
                .filter(|value| *value)
                .map(|_| NOTE_ACTIVE_ONLY.to_owned()),
        })
    }

    pub fn aether_contract_remove_logic(
        &self,
        request: AetherContractRemoveRequest,
    ) -> Result<AetherContractRemoveResponse, AetherMcpError> {
        self.state.require_writable()?;

        let store = self.state.store.as_ref();
        store
            .get_intent_contract(request.contract_id)?
            .ok_or_else(|| {
                AetherMcpError::Message(format!("contract #{} not found", request.contract_id))
            })?;
        store.deactivate_contract(request.contract_id)?;

        Ok(AetherContractRemoveResponse {
            contract_id: request.contract_id,
            status: "deactivated".to_owned(),
        })
    }

    pub fn aether_contract_check_logic(
        &self,
        request: AetherContractCheckRequest,
    ) -> Result<AetherContractCheckResponse, AetherMcpError> {
        let store = self.state.store.as_ref();
        let contracts = if let Some(symbol) = request.symbol.as_deref() {
            let symbol = resolve_symbol_selector(store, symbol)?;
            store.list_active_contracts_for_symbol(symbol.id.as_str())?
        } else {
            store.list_all_active_contracts()?
        };

        if contracts.is_empty() {
            return Ok(AetherContractCheckResponse {
                symbols_checked: Vec::new(),
                summary: ContractCheckSummary {
                    symbols_checked: 0,
                    total_clauses: 0,
                    passed: 0,
                    failed: 0,
                    ambiguous: 0,
                },
            });
        }

        let contracts_config = self.state.config.contracts.clone().unwrap_or_default();
        let verifier = ContractVerifier::from_config(&contracts_config);
        let mut grouped = BTreeMap::<String, Vec<IntentContractRecord>>::new();
        for contract in contracts {
            grouped
                .entry(contract.symbol_id.clone())
                .or_default()
                .push(contract);
        }

        let symbol_ids = grouped.keys().cloned().collect::<Vec<_>>();
        let qualified_names = qualified_names_by_symbol_id(store, &symbol_ids)?;

        let mut symbols_checked = Vec::new();
        let mut total_clauses = 0_u32;
        let mut total_passed = 0_u32;
        let mut total_failed = 0_u32;
        let mut total_ambiguous = 0_u32;

        for (symbol_id, contracts) in grouped {
            let sir_blob = store.read_sir_blob(symbol_id.as_str())?;
            let sir_embedding =
                load_symbol_embedding(store, self.state.config.as_ref(), symbol_id.as_str())?;
            let qualified_name = qualified_names.get(symbol_id.as_str()).cloned();

            let mut clause_results = Vec::with_capacity(contracts.len());
            let mut passed = 0_u32;
            let mut failed = 0_u32;
            let mut ambiguous = 0_u32;

            for contract in contracts {
                let mut invalid_clause_embedding = false;
                let clause_embedding = match contract.clause_embedding_json.as_deref() {
                    Some(json) => match serde_json::from_str::<Vec<f32>>(json) {
                        Ok(embedding) => Some(embedding),
                        Err(err) => {
                            tracing::warn!(
                                error = %err,
                                contract_id = contract.id,
                                symbol_id = symbol_id.as_str(),
                                "failed to parse stored contract clause embedding"
                            );
                            invalid_clause_embedding = true;
                            None
                        }
                    },
                    None => None,
                };

                let (status, similarity, note) = match sir_blob.as_ref() {
                    None => (ClauseStatus::Ambiguous, None, Some(NOTE_NO_SIR.to_owned())),
                    Some(_) => match (clause_embedding.as_deref(), sir_embedding.as_deref()) {
                        (Some(clause_embedding), Some(sir_embedding))
                            if embeddings_comparable(clause_embedding, sir_embedding) =>
                        {
                            let (status, similarity) =
                                verifier.classify_by_embedding(clause_embedding, sir_embedding);
                            (status, Some(similarity), None)
                        }
                        (Some(_), Some(_)) => (
                            ClauseStatus::Ambiguous,
                            None,
                            Some(NOTE_INCOMPATIBLE_EMBEDDING.to_owned()),
                        ),
                        _ => {
                            let note = if invalid_clause_embedding {
                                NOTE_INVALID_CLAUSE_EMBEDDING
                            } else {
                                NOTE_NO_EMBEDDING
                            };
                            (ClauseStatus::Ambiguous, None, Some(note.to_owned()))
                        }
                    },
                };

                match status {
                    ClauseStatus::Pass => passed += 1,
                    ClauseStatus::Fail => failed += 1,
                    ClauseStatus::Ambiguous => ambiguous += 1,
                }

                clause_results.push(ClauseResultOutput {
                    contract_id: contract.id,
                    clause_type: contract.clause_type,
                    clause_text: contract.clause_text,
                    status: clause_status_label(&status).to_owned(),
                    similarity,
                    judge_reason: None,
                    note,
                });
            }

            let clauses_checked = clause_results.len() as u32;
            total_clauses += clauses_checked;
            total_passed += passed;
            total_failed += failed;
            total_ambiguous += ambiguous;

            symbols_checked.push(SymbolCheckOutput {
                symbol_id,
                qualified_name,
                clauses_checked,
                passed,
                failed,
                ambiguous,
                clause_results,
            });
        }

        Ok(AetherContractCheckResponse {
            summary: ContractCheckSummary {
                symbols_checked: symbols_checked.len() as u32,
                total_clauses,
                passed: total_passed,
                failed: total_failed,
                ambiguous: total_ambiguous,
            },
            symbols_checked,
        })
    }

    pub fn aether_contract_violations_logic(
        &self,
        request: AetherContractViolationsRequest,
    ) -> Result<AetherContractViolationsResponse, AetherMcpError> {
        let store = self.state.store.as_ref();
        let limit = request
            .limit
            .unwrap_or(DEFAULT_VIOLATION_LIMIT)
            .clamp(1, MAX_VIOLATION_LIMIT) as usize;

        let violations = if let Some(symbol) = request.symbol.as_deref() {
            let symbol = resolve_symbol_selector(store, symbol)?;
            store.list_violations_for_symbol(symbol.id.as_str(), limit)?
        } else if let Some(contract_id) = request.contract_id {
            store.list_violations_for_contract(contract_id, limit)?
        } else {
            store.list_recent_violations(limit)?
        };

        let symbol_ids = violations
            .iter()
            .map(|violation| violation.symbol_id.clone())
            .collect::<Vec<_>>();
        let qualified_names = qualified_names_by_symbol_id(store, &symbol_ids)?;

        let outputs = violations
            .into_iter()
            .map(|violation| {
                let qualified_name = qualified_names.get(violation.symbol_id.as_str()).cloned();
                build_violation_output(violation, qualified_name)
            })
            .collect::<Vec<_>>();

        Ok(AetherContractViolationsResponse {
            total: outputs.len() as u32,
            violations: outputs,
        })
    }

    pub fn aether_contract_dismiss_logic(
        &self,
        request: AetherContractDismissRequest,
    ) -> Result<AetherContractDismissResponse, AetherMcpError> {
        self.state.require_writable()?;

        let reason = request.reason.trim();
        if reason.is_empty() {
            return Err(AetherMcpError::Message(
                "reason must not be empty".to_owned(),
            ));
        }
        if !violation_exists(self.sqlite_path().as_path(), request.violation_id)? {
            return Err(AetherMcpError::Message(format!(
                "violation #{} not found",
                request.violation_id
            )));
        }
        self.state
            .store
            .dismiss_violation(request.violation_id, reason)?;

        Ok(AetherContractDismissResponse {
            violation_id: request.violation_id,
            status: "dismissed".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{
        AetherContractAddRequest, AetherContractCheckRequest, AetherContractDismissRequest,
        AetherContractListRequest, AetherContractRemoveRequest, AetherContractViolationsRequest,
        CLAUDE_CODE_CREATOR, NOTE_ACTIVE_ONLY, NOTE_INVALID_CLAUSE_EMBEDDING, NOTE_NO_EMBEDDING,
    };
    use crate::AetherMcpServer;
    use aether_store::{
        SemanticIndexStore, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolEmbeddingRecord,
        SymbolRecord,
    };
    use tempfile::tempdir;

    fn write_test_config(workspace: &Path, embeddings_enabled: bool, embedding_model: &str) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = {embeddings_enabled}
provider = "qwen3_local"
vector_backend = "sqlite"
model = "{embedding_model}"
"#
            ),
        )
        .expect("write config");
    }

    fn seed_symbol(workspace: &Path, symbol_id: &str, qualified_name: &str) {
        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: symbol_id.to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: qualified_name.to_owned(),
                signature_fingerprint: format!("sig-{symbol_id}"),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
    }

    fn seed_sir_blob(workspace: &Path, symbol_id: &str, intent: &str) {
        let store = SqliteStore::open(workspace).expect("open store");
        store
            .write_sir_blob(
                symbol_id,
                format!(
                    r#"{{
                        "intent":"{intent}",
                        "inputs":[],
                        "outputs":[],
                        "side_effects":[],
                        "dependencies":[],
                        "error_modes":[],
                        "confidence":0.9
                    }}"#
                )
                .as_str(),
            )
            .expect("write sir blob");
    }

    fn seed_symbol_embedding(workspace: &Path, symbol_id: &str, embedding: Vec<f32>, model: &str) {
        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: symbol_id.to_owned(),
                sir_hash: format!("sir-{symbol_id}"),
                provider: "qwen3_local".to_owned(),
                model: model.to_owned(),
                embedding,
                updated_at: 1_700_000_000_000,
            })
            .expect("upsert symbol embedding");
    }

    #[test]
    fn contract_add_and_list_round_trip() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), false, "mock-64d");
        seed_symbol(temp.path(), "sym-contract", "crate::payments::process");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let add = server
            .aether_contract_add_logic(
                AetherContractAddRequest {
                    symbol: "crate::payments::process".to_owned(),
                    clause_type: "must".to_owned(),
                    clause_text: "reject zero amounts".to_owned(),
                    created_by: None,
                },
                None,
            )
            .expect("add contract");

        assert_eq!(add.status, "created");
        assert_eq!(add.symbol_id, "sym-contract");
        assert!(!add.has_embedding);

        let list = server
            .aether_contract_list_logic(AetherContractListRequest {
                symbol: Some("crate::payments::process".to_owned()),
                include_inactive: Some(true),
            })
            .expect("list contracts");

        assert_eq!(list.total, 1);
        assert_eq!(list.note.as_deref(), Some(NOTE_ACTIVE_ONLY));
        assert_eq!(list.contracts[0].id, add.contract_id);
        assert_eq!(
            list.contracts[0].qualified_name.as_deref(),
            Some("crate::payments::process")
        );
        assert_eq!(list.contracts[0].clause_type, "must");
        assert_eq!(list.contracts[0].clause_text, "reject zero amounts");
        assert_eq!(list.contracts[0].created_by, CLAUDE_CODE_CREATOR);
    }

    #[test]
    fn contract_add_validates_clause_type() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), false, "mock-64d");
        seed_symbol(temp.path(), "sym-contract", "crate::payments::process");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let err = server
            .aether_contract_add_logic(
                AetherContractAddRequest {
                    symbol: "sym-contract".to_owned(),
                    clause_type: "should".to_owned(),
                    clause_text: "reject zero amounts".to_owned(),
                    created_by: None,
                },
                None,
            )
            .expect_err("invalid clause type must fail");

        assert!(err.to_string().contains("clause_type must be one of"));
    }

    #[test]
    fn contract_remove_deactivates() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), false, "mock-64d");
        seed_symbol(temp.path(), "sym-contract", "crate::payments::process");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let add = server
            .aether_contract_add_logic(
                AetherContractAddRequest {
                    symbol: "sym-contract".to_owned(),
                    clause_type: "must".to_owned(),
                    clause_text: "reject zero amounts".to_owned(),
                    created_by: Some("tester".to_owned()),
                },
                None,
            )
            .expect("add contract");

        let remove = server
            .aether_contract_remove_logic(AetherContractRemoveRequest {
                contract_id: add.contract_id,
            })
            .expect("remove contract");
        assert_eq!(remove.status, "deactivated");

        let list = server
            .aether_contract_list_logic(AetherContractListRequest {
                symbol: Some("sym-contract".to_owned()),
                include_inactive: None,
            })
            .expect("list contracts");
        assert_eq!(list.total, 0);
        assert!(list.contracts.is_empty());
    }

    #[test]
    fn contract_check_passes_similar_sir() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), true, "mock-64d");
        seed_symbol(temp.path(), "sym-check", "crate::check::run");
        seed_sir_blob(temp.path(), "sym-check", "reject zero amounts");
        seed_symbol_embedding(temp.path(), "sym-check", vec![1.0, 0.0], "mock-64d");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let contract_embedding =
            serde_json::to_string(&vec![1.0_f32, 0.0]).expect("serialize embedding");
        store
            .insert_intent_contract(
                "sym-check",
                "must",
                "reject zero amounts",
                Some(contract_embedding.as_str()),
                "tester",
            )
            .expect("insert contract");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_contract_check_logic(AetherContractCheckRequest {
                symbol: Some("crate::check::run".to_owned()),
            })
            .expect("check contracts");

        assert_eq!(response.summary.symbols_checked, 1);
        assert_eq!(response.summary.total_clauses, 1);
        assert_eq!(response.summary.passed, 1);
        assert_eq!(response.summary.failed, 0);
        assert_eq!(response.summary.ambiguous, 0);
        assert_eq!(response.symbols_checked[0].clause_results[0].status, "pass");
        assert!(
            response.symbols_checked[0].clause_results[0]
                .similarity
                .expect("similarity")
                > 0.99
        );
    }

    #[test]
    fn contract_check_no_embedding_marks_ambiguous() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), true, "mock-64d");
        seed_symbol(temp.path(), "sym-check", "crate::check::run");
        seed_sir_blob(temp.path(), "sym-check", "reject zero amounts");
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .insert_intent_contract("sym-check", "must", "reject zero amounts", None, "tester")
            .expect("insert contract");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_contract_check_logic(AetherContractCheckRequest {
                symbol: Some("sym-check".to_owned()),
            })
            .expect("check contracts");

        assert_eq!(response.summary.total_clauses, 1);
        assert_eq!(response.summary.ambiguous, 1);
        assert_eq!(
            response.symbols_checked[0].clause_results[0].status,
            "ambiguous"
        );
        assert_eq!(
            response.symbols_checked[0].clause_results[0]
                .note
                .as_deref(),
            Some(NOTE_NO_EMBEDDING)
        );
    }

    #[test]
    fn contract_check_invalid_clause_embedding_marks_only_that_clause_ambiguous() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), true, "mock-64d");
        seed_symbol(temp.path(), "sym-check", "crate::check::run");
        seed_sir_blob(temp.path(), "sym-check", "reject zero amounts");
        seed_symbol_embedding(temp.path(), "sym-check", vec![1.0, 0.0], "mock-64d");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let valid_embedding =
            serde_json::to_string(&vec![1.0_f32, 0.0]).expect("serialize embedding");
        store
            .insert_intent_contract(
                "sym-check",
                "must",
                "reject zero amounts",
                Some(valid_embedding.as_str()),
                "tester",
            )
            .expect("insert valid contract");
        store
            .insert_intent_contract(
                "sym-check",
                "must_not",
                "panic on invalid input",
                Some("{not-json"),
                "tester",
            )
            .expect("insert malformed contract");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_contract_check_logic(AetherContractCheckRequest {
                symbol: Some("sym-check".to_owned()),
            })
            .expect("check contracts");

        assert_eq!(response.summary.total_clauses, 2);
        assert_eq!(response.summary.passed, 1);
        assert_eq!(response.summary.ambiguous, 1);
        assert_eq!(response.symbols_checked.len(), 1);
        assert_eq!(response.symbols_checked[0].clause_results[0].status, "pass");
        assert_eq!(
            response.symbols_checked[0].clause_results[1].status,
            "ambiguous"
        );
        assert_eq!(
            response.symbols_checked[0].clause_results[1]
                .note
                .as_deref(),
            Some(NOTE_INVALID_CLAUSE_EMBEDDING)
        );
    }

    #[test]
    fn contract_violations_returns_history() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), false, "mock-64d");
        seed_symbol(temp.path(), "sym-violation", "crate::violations::run");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let contract_id = store
            .insert_intent_contract(
                "sym-violation",
                "must",
                "reject zero amounts",
                None,
                "tester",
            )
            .expect("insert contract");
        let violation_id = store
            .insert_intent_violation(
                contract_id,
                "sym-violation",
                2,
                "embedding_fail",
                Some(0.42),
                Some("similarity below threshold"),
            )
            .expect("insert violation");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_contract_violations_logic(AetherContractViolationsRequest {
                symbol: Some("crate::violations::run".to_owned()),
                contract_id: None,
                limit: Some(10),
            })
            .expect("list violations");

        assert_eq!(response.total, 1);
        assert_eq!(response.violations[0].id, violation_id);
        assert_eq!(
            response.violations[0].qualified_name.as_deref(),
            Some("crate::violations::run")
        );
        assert_eq!(response.violations[0].violation_type, "embedding_fail");
        assert_eq!(response.violations[0].confidence, Some(0.42));
    }

    #[test]
    fn contract_dismiss_sets_fields() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), false, "mock-64d");
        seed_symbol(temp.path(), "sym-violation", "crate::violations::run");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let contract_id = store
            .insert_intent_contract(
                "sym-violation",
                "must",
                "reject zero amounts",
                None,
                "tester",
            )
            .expect("insert contract");
        let violation_id = store
            .insert_intent_violation(
                contract_id,
                "sym-violation",
                2,
                "embedding_fail",
                None,
                None,
            )
            .expect("insert violation");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let response = server
            .aether_contract_dismiss_logic(AetherContractDismissRequest {
                violation_id,
                reason: "expected semantic drift during refactor".to_owned(),
            })
            .expect("dismiss violation");
        assert_eq!(response.status, "dismissed");

        let violations = store
            .list_violations_for_contract(contract_id, 10)
            .expect("list violations");
        assert_eq!(violations.len(), 1);
        assert!(violations[0].dismissed);
        assert_eq!(
            violations[0].dismissed_reason.as_deref(),
            Some("expected semantic drift during refactor")
        );
    }

    #[test]
    fn contract_dismiss_rejects_unknown_violation_id() {
        let temp = tempdir().expect("tempdir");
        write_test_config(temp.path(), false, "mock-64d");
        let server = AetherMcpServer::new(temp.path(), false).expect("server");

        let err = server
            .aether_contract_dismiss_logic(AetherContractDismissRequest {
                violation_id: 9999,
                reason: "not real".to_owned(),
            })
            .expect_err("unknown violation id must fail");

        assert!(err.to_string().contains("violation #9999 not found"));
    }
}
