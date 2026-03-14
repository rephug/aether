use std::sync::Arc;

use aether_infer::{
    EmbeddingProviderOverrides, EmbeddingPurpose, load_embedding_provider_from_config,
};
use aether_memory::{
    AskInclude as MemoryAskInclude, AskQueryRequest as MemoryAskQueryRequest,
    EntityRef as MemoryEntityRef, NoteEmbeddingRequest as MemoryNoteEmbeddingRequest,
    NoteSourceType as MemoryNoteSourceType, ProjectMemoryService,
    RecallRequest as MemoryRecallRequest, RememberRequest as MemoryRememberRequest,
    SemanticQuery as MemorySemanticQuery, truncate_content_for_embedding,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AetherMcpServer, MEMORY_SCHEMA_VERSION, effective_limit};
use crate::{AetherMcpError, SearchMode};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherMemoryEntityRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRememberRequest {
    pub content: String,
    pub tags: Option<Vec<String>>,
    pub entity_refs: Option<Vec<AetherMemoryEntityRef>>,
    pub file_refs: Option<Vec<String>>,
    pub symbol_refs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRememberResponse {
    pub schema_version: String,
    pub note_id: String,
    pub action: String,
    pub content_hash: String,
    pub tags: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSessionNoteResponse {
    pub schema_version: String,
    pub note_id: String,
    pub action: String,
    pub source_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRecallRequest {
    pub query: String,
    pub mode: Option<SearchMode>,
    pub limit: Option<u32>,
    pub include_archived: Option<bool>,
    pub tags_filter: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRecallNote {
    pub note_id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub file_refs: Vec<String>,
    pub symbol_refs: Vec<String>,
    pub source_type: String,
    pub created_at: i64,
    pub access_count: i64,
    pub relevance_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherRecallResponse {
    pub schema_version: String,
    pub query: String,
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub result_count: u32,
    pub notes: Vec<AetherRecallNote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherAskInclude {
    Symbols,
    Notes,
    Coupling,
    Tests,
}

impl From<AetherAskInclude> for MemoryAskInclude {
    fn from(value: AetherAskInclude) -> Self {
        match value {
            AetherAskInclude::Symbols => MemoryAskInclude::Symbols,
            AetherAskInclude::Notes => MemoryAskInclude::Notes,
            AetherAskInclude::Coupling => MemoryAskInclude::Coupling,
            AetherAskInclude::Tests => MemoryAskInclude::Tests,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAskRequest {
    pub query: String,
    pub limit: Option<u32>,
    pub include: Option<Vec<AetherAskInclude>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AetherAskKind {
    Symbol,
    Note,
    TestGuard,
    CoupledFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAskResult {
    pub kind: AetherAskKind,
    pub id: Option<String>,
    pub title: Option<String>,
    pub snippet: String,
    pub relevance_score: f32,
    pub file: Option<String>,
    pub language: Option<String>,
    pub tags: Option<Vec<String>>,
    pub source_type: Option<String>,
    pub test_file: Option<String>,
    pub fused_score: Option<f32>,
    pub coupling_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherAskResponse {
    pub schema_version: String,
    pub query: String,
    pub result_count: u32,
    pub results: Vec<AetherAskResult>,
}

impl AetherMcpServer {
    async fn remember_note_with_source_type(
        &self,
        request: AetherRememberRequest,
        source_type: MemoryNoteSourceType,
    ) -> Result<aether_memory::RememberResult, AetherMcpError> {
        let memory = ProjectMemoryService::with_shared(
            self.workspace(),
            Arc::clone(&self.state.store),
            self.state.vector_store.clone(),
        );
        let entity_refs = request
            .entity_refs
            .unwrap_or_default()
            .into_iter()
            .map(|entity| MemoryEntityRef {
                kind: entity.kind,
                id: entity.id,
            })
            .collect::<Vec<_>>();

        let remember = memory.remember(MemoryRememberRequest {
            content: request.content,
            source_type,
            source_agent: Some("aether_mcp".to_owned()),
            tags: request.tags.unwrap_or_default(),
            entity_refs,
            file_refs: request.file_refs.unwrap_or_default(),
            symbol_refs: request.symbol_refs.unwrap_or_default(),
            now_ms: None,
        })?;

        if remember.action == aether_memory::RememberAction::Created {
            match load_embedding_provider_from_config(
                self.workspace(),
                EmbeddingProviderOverrides::default(),
            ) {
                Ok(Some(loaded)) => {
                    let content = truncate_content_for_embedding(remember.note.content.as_str());
                    match loaded
                        .provider
                        .embed_text_with_purpose(content.as_str(), EmbeddingPurpose::Document)
                        .await
                    {
                        Ok(embedding) if !embedding.is_empty() => {
                            if let Err(err) = memory
                                .upsert_note_embedding(MemoryNoteEmbeddingRequest {
                                    note_id: remember.note.note_id.clone(),
                                    provider: loaded.provider_name,
                                    model: loaded.model_name,
                                    embedding,
                                    content: remember.note.content.clone(),
                                    created_at: remember.note.created_at,
                                    updated_at: Some(remember.note.updated_at),
                                })
                                .await
                            {
                                tracing::warn!(
                                    error = %err,
                                    "failed to persist note embedding after remember"
                                );
                            }
                        }
                        Ok(_) => tracing::warn!(
                            "embedding provider returned empty vector while indexing project note"
                        ),
                        Err(err) => tracing::warn!(
                            error = %err,
                            "embedding provider error while indexing project note"
                        ),
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(
                    error = %err,
                    "failed to load embedding provider for project note indexing"
                ),
            }
        }

        Ok(remember)
    }

    pub async fn aether_remember_logic(
        &self,
        request: AetherRememberRequest,
    ) -> Result<AetherRememberResponse, AetherMcpError> {
        self.state.require_writable()?;
        let remember = self
            .remember_note_with_source_type(request, MemoryNoteSourceType::Agent)
            .await?;

        Ok(AetherRememberResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            note_id: remember.note.note_id,
            action: remember.action.as_str().to_owned(),
            content_hash: remember.note.content_hash,
            tags: remember.note.tags,
            created_at: remember.note.created_at,
        })
    }

    pub async fn aether_session_note_logic(
        &self,
        request: AetherRememberRequest,
    ) -> Result<AetherSessionNoteResponse, AetherMcpError> {
        self.state.require_writable()?;
        let remember = self
            .remember_note_with_source_type(request, MemoryNoteSourceType::Session)
            .await?;

        Ok(AetherSessionNoteResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            note_id: remember.note.note_id,
            action: remember.action.as_str().to_owned(),
            source_type: MemoryNoteSourceType::Session.as_str().to_owned(),
        })
    }

    pub async fn aether_recall_logic(
        &self,
        request: AetherRecallRequest,
    ) -> Result<AetherRecallResponse, AetherMcpError> {
        let mode = request.mode.unwrap_or(SearchMode::Hybrid);
        let limit = request.limit.unwrap_or(5).clamp(1, 100);

        let mut semantic_query = None;
        let mut semantic_fallback_reason = None;
        if !matches!(mode, SearchMode::Lexical) {
            match load_embedding_provider_from_config(
                self.workspace(),
                EmbeddingProviderOverrides::default(),
            ) {
                Ok(Some(loaded)) => {
                    match loaded
                        .provider
                        .embed_text_with_purpose(request.query.as_str(), EmbeddingPurpose::Query)
                        .await
                    {
                        Ok(embedding) if !embedding.is_empty() => {
                            semantic_query = Some(MemorySemanticQuery {
                                provider: loaded.provider_name,
                                model: loaded.model_name,
                                embedding,
                            });
                        }
                        Ok(_) => {
                            semantic_fallback_reason = Some(
                                aether_core::SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR
                                    .to_owned(),
                            )
                        }
                        Err(err) => {
                            semantic_fallback_reason =
                                Some(format!("embedding provider error: {err}"))
                        }
                    }
                }
                Ok(None) => {
                    semantic_fallback_reason =
                        Some(aether_core::SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned())
                }
                Err(err) => {
                    semantic_fallback_reason =
                        Some(format!("failed to load embedding provider: {err}"))
                }
            }
        }

        let memory = ProjectMemoryService::with_shared(
            self.workspace(),
            Arc::clone(&self.state.store),
            self.state.vector_store.clone(),
        );
        let result = memory
            .recall(MemoryRecallRequest {
                query: request.query.clone(),
                mode,
                limit,
                include_archived: request.include_archived.unwrap_or(false),
                tags_filter: request.tags_filter.unwrap_or_default(),
                now_ms: None,
                semantic: semantic_query,
                semantic_fallback_reason,
            })
            .await?;

        let notes = result
            .notes
            .into_iter()
            .map(|entry| AetherRecallNote {
                note_id: entry.note.note_id,
                content: entry.note.content,
                tags: entry.note.tags,
                file_refs: entry.note.file_refs,
                symbol_refs: entry.note.symbol_refs,
                source_type: entry.note.source_type,
                created_at: entry.note.created_at,
                access_count: entry.note.access_count,
                relevance_score: entry.relevance_score,
            })
            .collect::<Vec<_>>();
        let result_count = notes.len() as u32;

        Ok(AetherRecallResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            query: request.query,
            mode_requested: result.mode_requested,
            mode_used: result.mode_used,
            fallback_reason: result.fallback_reason,
            result_count,
            notes,
        })
    }

    pub async fn aether_ask_logic(
        &self,
        request: AetherAskRequest,
    ) -> Result<AetherAskResponse, AetherMcpError> {
        let limit = effective_limit(request.limit);
        let include = request
            .include
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();

        let mut semantic_query = None;
        match load_embedding_provider_from_config(
            self.workspace(),
            EmbeddingProviderOverrides::default(),
        ) {
            Ok(Some(loaded)) => match loaded
                .provider
                .embed_text_with_purpose(request.query.as_str(), EmbeddingPurpose::Query)
                .await
            {
                Ok(embedding) if !embedding.is_empty() => {
                    semantic_query = Some(MemorySemanticQuery {
                        provider: loaded.provider_name,
                        model: loaded.model_name,
                        embedding,
                    });
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(error = %err, "embedding provider error while handling aether_ask");
                }
            },
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(error = %err, "failed to load embedding provider for aether_ask");
            }
        }

        let memory = ProjectMemoryService::with_shared(
            self.workspace(),
            Arc::clone(&self.state.store),
            self.state.vector_store.clone(),
        );
        let result = memory
            .ask(MemoryAskQueryRequest {
                query: request.query.clone(),
                limit,
                include,
                now_ms: None,
                semantic: semantic_query,
            })
            .await?;

        let results = result
            .results
            .into_iter()
            .map(|entry| AetherAskResult {
                kind: match entry.kind {
                    aether_memory::AskResultKind::Symbol => AetherAskKind::Symbol,
                    aether_memory::AskResultKind::Note => AetherAskKind::Note,
                    aether_memory::AskResultKind::TestGuard => AetherAskKind::TestGuard,
                    aether_memory::AskResultKind::CoupledFile => AetherAskKind::CoupledFile,
                },
                id: entry.id,
                title: entry.title,
                snippet: entry.snippet,
                relevance_score: entry.relevance_score,
                file: entry.file,
                language: entry.language,
                tags: (!entry.tags.is_empty()).then_some(entry.tags),
                source_type: entry.source_type,
                test_file: entry.test_file,
                fused_score: entry.fused_score,
                coupling_type: entry.coupling_type,
            })
            .collect::<Vec<_>>();

        Ok(AetherAskResponse {
            schema_version: MEMORY_SCHEMA_VERSION.to_owned(),
            query: result.query,
            result_count: results.len() as u32,
            results,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use aether_store::{
        CouplingEdgeRecord, CozoGraphStore, ProjectNoteRecord, ProjectNoteStore, SirStateStore,
        SymbolCatalogStore, SymbolRecord, TestIntentRecord, TestIntentStore,
    };
    use tempfile::tempdir;

    use super::{AetherAskKind, AetherAskRequest};
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

    #[tokio::test]
    async fn aether_ask_returns_mixed_result_types() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);
        let server = AetherMcpServer::init(workspace, false)
            .await
            .expect("new mcp server");
        let store = aether_store::SqliteStore::open(workspace).expect("open store");

        store
            .upsert_symbol(SymbolRecord {
                id: "sym-payment".to_owned(),
                file_path: "src/payments/processor.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "process_payment_with_retry".to_owned(),
                signature_fingerprint: "sig-payment".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");
        store
            .write_sir_blob(
                "sym-payment",
                r#"{
                    "intent":"Processes payment retries with capped backoff",
                    "inputs":[],
                    "outputs":[],
                    "side_effects":[],
                    "dependencies":[],
                    "error_modes":[],
                    "confidence":0.9
                }"#,
            )
            .expect("write sir");

        store
            .upsert_project_note(ProjectNoteRecord {
                note_id: "note-payment".to_owned(),
                content: "Refactored payment retry workflow for timeout spikes".to_owned(),
                content_hash: "note-hash-payment".to_owned(),
                source_type: "session".to_owned(),
                source_agent: Some("test".to_owned()),
                tags: vec!["refactor".to_owned()],
                entity_refs: Vec::new(),
                file_refs: vec!["src/payments/processor.rs".to_owned()],
                symbol_refs: vec!["sym-payment".to_owned()],
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
                access_count: 0,
                last_accessed_at: None,
                is_archived: false,
            })
            .expect("upsert note");

        store
            .replace_test_intents_for_file(
                "tests/payment_test.rs",
                &[TestIntentRecord {
                    intent_id: "intent-timeout".to_owned(),
                    file_path: "tests/payment_test.rs".to_owned(),
                    test_name: "test_retry_timeout".to_owned(),
                    intent_text: "retries payment timeout".to_owned(),
                    group_label: None,
                    language: "rust".to_owned(),
                    symbol_id: Some("sym-payment".to_owned()),
                    created_at: 1_700_000_000_000,
                    updated_at: 1_700_000_000_100,
                }],
            )
            .expect("upsert test intent");

        let cozo = CozoGraphStore::open(workspace).expect("open cozo");
        cozo.upsert_co_change_edges(&[CouplingEdgeRecord {
            file_a: "src/payments/processor.rs".to_owned(),
            file_b: "src/payments/gateway.rs".to_owned(),
            co_change_count: 8,
            total_commits_a: 10,
            total_commits_b: 9,
            git_coupling: 0.85,
            static_signal: 0.7,
            semantic_signal: 0.6,
            fused_score: 0.8,
            coupling_type: "multi".to_owned(),
            last_co_change_commit: "abc123".to_owned(),
            last_co_change_at: 1_700_000_000,
            mined_at: 1_700_000_100,
        }])
        .expect("upsert co-change");

        let response = server
            .aether_ask_logic(AetherAskRequest {
                query: "payment".to_owned(),
                limit: Some(10),
                include: None,
            })
            .await
            .expect("aether ask");

        let kinds = response
            .results
            .iter()
            .map(|item| item.kind)
            .collect::<Vec<_>>();
        assert!(kinds.contains(&AetherAskKind::Symbol));
        assert!(kinds.contains(&AetherAskKind::Note));
        assert!(kinds.contains(&AetherAskKind::TestGuard));
        assert!(response.result_count >= 3);
    }
}
