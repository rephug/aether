use std::collections::{HashMap, HashSet};
use std::path::Path;

use aether_analysis::{
    BlastRadiusRequest, CouplingAnalyzer, RiskLevel as CouplingRiskLevel, TestIntentAnalyzer,
};
use aether_core::normalize_path;
use aether_store::{ProjectNoteStore, SqliteStore, SymbolCatalogStore, TestIntentStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, MEMORY_SCHEMA_VERSION, current_unix_timestamp_millis};
use crate::AetherMcpError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherCouplingRiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl From<AetherCouplingRiskLevel> for CouplingRiskLevel {
    fn from(value: AetherCouplingRiskLevel) -> Self {
        match value {
            AetherCouplingRiskLevel::Low => CouplingRiskLevel::Low,
            AetherCouplingRiskLevel::Medium => CouplingRiskLevel::Medium,
            AetherCouplingRiskLevel::High => CouplingRiskLevel::High,
            AetherCouplingRiskLevel::Critical => CouplingRiskLevel::Critical,
        }
    }
}

impl From<CouplingRiskLevel> for AetherCouplingRiskLevel {
    fn from(value: CouplingRiskLevel) -> Self {
        match value {
            CouplingRiskLevel::Low => AetherCouplingRiskLevel::Low,
            CouplingRiskLevel::Medium => AetherCouplingRiskLevel::Medium,
            CouplingRiskLevel::High => AetherCouplingRiskLevel::High,
            CouplingRiskLevel::Critical => AetherCouplingRiskLevel::Critical,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBlastRadiusRequest {
    pub file: String,
    pub min_risk: Option<AetherCouplingRiskLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBlastRadiusMiningState {
    pub commits_scanned: i64,
    pub last_mined_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBlastRadiusSignals {
    pub temporal: f32,
    pub static_signal: f32,
    pub semantic: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBlastRadiusCoupledFile {
    pub file: String,
    pub risk_level: AetherCouplingRiskLevel,
    pub fused_score: f32,
    pub coupling_type: String,
    pub signals: AetherBlastRadiusSignals,
    pub co_change_count: i64,
    pub total_commits: i64,
    pub last_co_change: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBlastRadiusResponse {
    pub schema_version: String,
    pub target_file: String,
    pub mining_state: Option<AetherBlastRadiusMiningState>,
    pub coupled_files: Vec<AetherBlastRadiusCoupledFile>,
    pub test_guards: Vec<AetherBlastRadiusTestGuard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherBlastRadiusTestGuard {
    pub test_file: String,
    pub intents: Vec<String>,
    pub confidence: f32,
    pub inference_method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTestIntentsRequest {
    pub file: Option<String>,
    pub symbol_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTestIntentEntry {
    pub intent_id: String,
    pub file_path: String,
    pub test_name: String,
    pub intent_text: String,
    pub group_label: Option<String>,
    pub language: String,
    pub symbol_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTestIntentsResponse {
    pub schema_version: String,
    pub file: Option<String>,
    pub symbol_id: Option<String>,
    pub result_count: u32,
    pub intents: Vec<AetherTestIntentEntry>,
}

impl AetherMcpServer {
    pub fn aether_blast_radius_logic(
        &self,
        request: AetherBlastRadiusRequest,
    ) -> Result<AetherBlastRadiusResponse, AetherMcpError> {
        let target_file = normalize_path(request.file.trim());
        if target_file.is_empty() {
            return Ok(AetherBlastRadiusResponse {
                schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
                target_file,
                mining_state: None,
                coupled_files: Vec::new(),
                test_guards: Vec::new(),
            });
        }

        let analyzer = CouplingAnalyzer::new(self.workspace())?;
        let blast = analyzer.blast_radius(BlastRadiusRequest {
            file_path: target_file.clone(),
            min_risk: request
                .min_risk
                .unwrap_or(AetherCouplingRiskLevel::Medium)
                .into(),
            auto_mine: !self.state.read_only,
        })?;

        let mining_state = blast
            .mining_state
            .map(|state| AetherBlastRadiusMiningState {
                commits_scanned: state.commits_scanned,
                last_mined_at: state.last_mined_at,
            });

        let store = self.state.store.as_ref();
        let target_symbol_ids = store
            .list_symbols_for_file(blast.target_file.as_str())?
            .into_iter()
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>();
        store.increment_symbol_access(
            target_symbol_ids.as_slice(),
            current_unix_timestamp_millis(),
        )?;

        let mut coupled_files = Vec::with_capacity(blast.coupled_files.len());
        for entry in blast.coupled_files {
            let notes = store
                .list_project_notes_for_file_ref(entry.file.as_str(), 5)?
                .into_iter()
                .map(|note| note.content)
                .collect::<Vec<_>>();

            coupled_files.push(AetherBlastRadiusCoupledFile {
                file: entry.file,
                risk_level: entry.risk_level.into(),
                fused_score: entry.fused_score,
                coupling_type: entry.coupling_type.as_str().to_owned(),
                signals: AetherBlastRadiusSignals {
                    temporal: entry.signals.temporal,
                    static_signal: entry.signals.static_signal,
                    semantic: entry.signals.semantic,
                },
                co_change_count: entry.co_change_count,
                total_commits: entry.total_commits,
                last_co_change: entry.last_co_change_commit,
                notes,
            });
        }

        let test_guards = match TestIntentAnalyzer::new(self.workspace())
            .and_then(|analyzer| analyzer.list_guards_for_target_file(blast.target_file.as_str()))
        {
            Ok(guards) => {
                let mapped = guards
                    .into_iter()
                    .map(|guard| AetherBlastRadiusTestGuard {
                        test_file: guard.test_file,
                        intents: guard.intents,
                        confidence: guard.confidence,
                        inference_method: guard.inference_method,
                    })
                    .collect::<Vec<_>>();

                if mapped.is_empty() {
                    self.fallback_test_guards_from_naming(store, blast.target_file.as_str())?
                } else {
                    mapped
                }
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    target_file = %blast.target_file,
                    "falling back to naming-based test guard inference"
                );
                self.fallback_test_guards_from_naming(store, blast.target_file.as_str())?
            }
        };

        Ok(AetherBlastRadiusResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            target_file: blast.target_file,
            mining_state,
            coupled_files,
            test_guards,
        })
    }

    pub fn aether_test_intents_logic(
        &self,
        request: AetherTestIntentsRequest,
    ) -> Result<AetherTestIntentsResponse, AetherMcpError> {
        let file = request.file.map(|value| normalize_path(value.trim()));
        let symbol_id = request
            .symbol_id
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());

        if file
            .as_deref()
            .map(|value| value.is_empty())
            .unwrap_or(true)
            && symbol_id.is_none()
        {
            return Ok(AetherTestIntentsResponse {
                schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
                file,
                symbol_id,
                result_count: 0,
                intents: Vec::new(),
            });
        }

        let store = self.state.store.as_ref();
        let mut by_id = HashMap::<String, AetherTestIntentEntry>::new();

        if let Some(file_path) = file.as_deref()
            && !file_path.is_empty()
        {
            for intent in store.list_test_intents_for_file(file_path)? {
                by_id.insert(
                    intent.intent_id.clone(),
                    AetherTestIntentEntry {
                        intent_id: intent.intent_id,
                        file_path: intent.file_path,
                        test_name: intent.test_name,
                        intent_text: intent.intent_text,
                        group_label: intent.group_label,
                        language: intent.language,
                        symbol_id: intent.symbol_id,
                    },
                );
            }
        }

        if let Some(symbol) = symbol_id.as_deref() {
            for intent in store.list_test_intents_for_symbol(symbol)? {
                by_id.insert(
                    intent.intent_id.clone(),
                    AetherTestIntentEntry {
                        intent_id: intent.intent_id,
                        file_path: intent.file_path,
                        test_name: intent.test_name,
                        intent_text: intent.intent_text,
                        group_label: intent.group_label,
                        language: intent.language,
                        symbol_id: intent.symbol_id,
                    },
                );
            }
        }

        let mut intents = by_id.into_values().collect::<Vec<_>>();
        intents.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then_with(|| left.test_name.cmp(&right.test_name))
                .then_with(|| left.intent_id.cmp(&right.intent_id))
        });

        Ok(AetherTestIntentsResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            file,
            symbol_id,
            result_count: intents.len() as u32,
            intents,
        })
    }

    fn fallback_test_guards_from_naming(
        &self,
        store: &SqliteStore,
        target_file: &str,
    ) -> Result<Vec<AetherBlastRadiusTestGuard>, AetherMcpError> {
        let target_file = normalize_path(target_file.trim());
        if target_file.is_empty() {
            return Ok(Vec::new());
        }

        let mut candidates = HashSet::new();
        if let Some((root, tail)) = split_source_root(target_file.as_str()) {
            let source_path = Path::new(tail.as_str());
            let stem = source_path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let ext = source_path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default();

            if !stem.is_empty() && !ext.is_empty() {
                let root = if root.is_empty() {
                    String::new()
                } else {
                    format!("{root}/")
                };
                candidates.insert(format!("{root}tests/{stem}_test.{ext}"));
                candidates.insert(format!("{root}tests/{stem}_tests.{ext}"));
                candidates.insert(format!("{root}src/{stem}.test.{ext}"));
                candidates.insert(format!("{root}src/{stem}.spec.{ext}"));
                candidates.insert(format!("{root}src/__tests__/{stem}.{ext}"));
            }
        }

        let mut guards = Vec::new();
        for candidate in candidates {
            let intents = store
                .list_test_intents_for_file(candidate.as_str())?
                .into_iter()
                .map(|intent| intent.intent_text)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if intents.is_empty() {
                continue;
            }
            guards.push(AetherBlastRadiusTestGuard {
                test_file: candidate,
                intents,
                confidence: 0.9,
                inference_method: "naming_convention".to_owned(),
            });
        }

        guards.sort_by(|left, right| left.test_file.cmp(&right.test_file));
        Ok(guards)
    }
}

fn split_source_root(path: &str) -> Option<(String, String)> {
    if let Some(tail) = path.strip_prefix("src/") {
        return Some((String::new(), tail.to_owned()));
    }
    let (root, tail) = path.split_once("/src/")?;
    Some((root.to_owned(), tail.to_owned()))
}
