use aether_core::EdgeKind;
use aether_health::{
    ConsumerMethodUsage, CrossCuttingMethod, SplitConfidence, SuggestedSubTrait, TraitMethod,
    TraitSplitSuggestion, suggest_trait_split,
};
use aether_store::{SqliteStore, SymbolCatalogStore, SymbolRecord, SymbolRelationStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, MEMORY_SCHEMA_VERSION, child_method_symbols, symbol_leaf_name};
use crate::AetherMcpError;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSuggestTraitSplitRequest {
    pub trait_name: String,
    pub file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSuggestTraitSplitResponse {
    pub schema_version: String,
    pub resolved_via: AetherTraitSplitResolvedVia,
    pub suggestion: Option<AetherTraitSplitSuggestion>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherSplitConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherTraitSplitResolutionMode {
    Direct,
    Implementor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraitSplitResolvedVia {
    pub mode: AetherTraitSplitResolutionMode,
    pub symbol_id: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherTraitSplitSuggestion {
    pub trait_name: String,
    pub trait_file: String,
    pub method_count: usize,
    pub suggested_traits: Vec<AetherSuggestedSubTrait>,
    pub cross_cutting_methods: Vec<AetherCrossCuttingMethod>,
    pub uncalled_methods: Vec<String>,
    pub confidence: AetherSplitConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSuggestedSubTrait {
    pub name: String,
    pub methods: Vec<String>,
    pub consumer_files: Vec<String>,
    pub consumer_isolation: f32,
    pub dominant_dependencies: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCrossCuttingMethod {
    pub method: String,
    pub overlapping_clusters: Vec<String>,
    pub reason: String,
}

impl AetherMcpServer {
    pub fn aether_suggest_trait_split_logic(
        &self,
        request: AetherSuggestTraitSplitRequest,
    ) -> Result<AetherSuggestTraitSplitResponse, AetherMcpError> {
        let trait_name = request.trait_name.trim();
        if trait_name.is_empty() {
            return Err(AetherMcpError::Message(
                "trait_name must not be empty".to_owned(),
            ));
        }

        let file_filter = self.normalize_usage_matrix_file(request.file.as_deref())?;
        let target =
            self.resolve_usage_matrix_target(trait_name, None, file_filter.as_deref(), None)?;

        let store = self.state.store.as_ref();
        let Some(target_record) = store.get_symbol_record(target.symbol_id.as_str())? else {
            return Err(AetherMcpError::Message(format!(
                "symbol '{}' could not be resolved after selection",
                target.symbol_id
            )));
        };
        if !matches!(target_record.kind.as_str(), "trait" | "struct") {
            return Err(AetherMcpError::Message(format!(
                "symbol '{}' resolved to kind '{}'; expected trait or struct",
                target_record.qualified_name, target_record.kind
            )));
        }

        let (method_source_record, resolved_via) =
            self.resolve_trait_split_method_source(store, &target_record)?;
        let data = self.build_usage_matrix_data(store, &method_source_record)?;
        let methods = data
            .child_methods
            .iter()
            .map(|method| TraitMethod {
                name: symbol_leaf_name(method.qualified_name.as_str()).to_owned(),
                qualified_name: method.qualified_name.clone(),
                symbol_id: method.id.clone(),
            })
            .collect::<Vec<_>>();
        let consumer_matrix = data
            .matrix
            .iter()
            .map(|row| ConsumerMethodUsage {
                consumer_file: row.consumer_file.clone(),
                methods_used: row.methods_used.clone(),
            })
            .collect::<Vec<_>>();
        let method_dependencies = self
            .read_valid_sir_blob(method_source_record.id.as_str())?
            .and_then(|sir| sir.method_dependencies);

        let suggestion = suggest_trait_split(
            symbol_leaf_name(target_record.qualified_name.as_str()),
            target_record.file_path.as_str(),
            methods.as_slice(),
            consumer_matrix.as_slice(),
            method_dependencies.as_ref(),
        );

        let message = if suggestion.is_none() {
            Some("no actionable trait split suggestion was produced".to_owned())
        } else {
            None
        };

        Ok(AetherSuggestTraitSplitResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            resolved_via,
            suggestion: suggestion.map(Into::into),
            message,
        })
    }

    fn resolve_trait_split_method_source(
        &self,
        store: &SqliteStore,
        target_record: &SymbolRecord,
    ) -> Result<(SymbolRecord, AetherTraitSplitResolvedVia), AetherMcpError> {
        if target_record.kind != "trait" || !child_method_symbols(store, target_record)?.is_empty()
        {
            return Ok((
                target_record.clone(),
                trait_split_resolution(target_record, AetherTraitSplitResolutionMode::Direct),
            ));
        }

        let mut candidates = store
            .list_symbols_for_file(target_record.file_path.as_str())?
            .into_iter()
            .filter(|candidate| candidate.id != target_record.id && candidate.kind == "struct")
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            left.qualified_name
                .cmp(&right.qualified_name)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut fallback = None;
        for candidate in candidates {
            if child_method_symbols(store, &candidate)?.is_empty() {
                continue;
            }

            let implements_target = store
                .get_dependencies(candidate.id.as_str())?
                .into_iter()
                .any(|edge| {
                    matches!(edge.edge_kind, EdgeKind::Implements)
                        && edge.target_qualified_name == target_record.qualified_name
                });
            if implements_target {
                return Ok((
                    candidate.clone(),
                    trait_split_resolution(&candidate, AetherTraitSplitResolutionMode::Implementor),
                ));
            }

            if fallback.is_none() {
                fallback = Some(candidate);
            }
        }

        if let Some(candidate) = fallback {
            return Ok((
                candidate.clone(),
                trait_split_resolution(&candidate, AetherTraitSplitResolutionMode::Implementor),
            ));
        }

        Ok((
            target_record.clone(),
            trait_split_resolution(target_record, AetherTraitSplitResolutionMode::Direct),
        ))
    }
}

impl From<SplitConfidence> for AetherSplitConfidence {
    fn from(value: SplitConfidence) -> Self {
        match value {
            SplitConfidence::High => Self::High,
            SplitConfidence::Medium => Self::Medium,
            SplitConfidence::Low => Self::Low,
        }
    }
}

impl From<TraitSplitSuggestion> for AetherTraitSplitSuggestion {
    fn from(value: TraitSplitSuggestion) -> Self {
        Self {
            trait_name: value.trait_name,
            trait_file: value.trait_file,
            method_count: value.method_count,
            suggested_traits: value.suggested_traits.into_iter().map(Into::into).collect(),
            cross_cutting_methods: value
                .cross_cutting_methods
                .into_iter()
                .map(Into::into)
                .collect(),
            uncalled_methods: value.uncalled_methods,
            confidence: value.confidence.into(),
        }
    }
}

impl From<SuggestedSubTrait> for AetherSuggestedSubTrait {
    fn from(value: SuggestedSubTrait) -> Self {
        Self {
            name: value.name,
            methods: value.methods,
            consumer_files: value.consumer_files,
            consumer_isolation: value.consumer_isolation,
            dominant_dependencies: value.dominant_dependencies,
            reason: value.reason,
        }
    }
}

impl From<CrossCuttingMethod> for AetherCrossCuttingMethod {
    fn from(value: CrossCuttingMethod) -> Self {
        Self {
            method: value.method,
            overlapping_clusters: value.overlapping_clusters,
            reason: value.reason,
        }
    }
}

fn trait_split_resolution(
    record: &SymbolRecord,
    mode: AetherTraitSplitResolutionMode,
) -> AetherTraitSplitResolvedVia {
    AetherTraitSplitResolvedVia {
        mode,
        symbol_id: record.id.clone(),
        qualified_name: record.qualified_name.clone(),
        kind: record.kind.clone(),
        file_path: record.file_path.clone(),
    }
}
