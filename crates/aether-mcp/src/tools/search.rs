use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aether_config::SearchRerankerKind;
use aether_core::{
    SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR, SEARCH_FALLBACK_EMBEDDINGS_DISABLED,
    SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED, SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY,
    SearchEnvelope,
};
use aether_infer::{
    EmbeddingProviderOverrides, EmbeddingPurpose, RerankCandidate, RerankerProvider,
    RerankerProviderOverrides, load_embedding_provider_from_config,
    load_reranker_provider_from_config,
};
use aether_store::{
    SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord, SymbolSearchResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AetherMcpServer, child_method_symbols, effective_limit, is_type_symbol_kind};
use crate::state::semantic_search_unavailability;
use crate::{AetherMcpError, SearchMode};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupRequest {
    pub query: String,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupMatch {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupResponse {
    pub query: String,
    pub limit: u32,
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub result_count: u32,
    pub matches: Vec<AetherSymbolLookupMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDependenciesRequest {
    pub symbol_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDependenciesResponse {
    pub symbol_id: String,
    pub found: bool,
    pub aggregated: bool,
    pub child_method_count: u32,
    pub caller_count: u32,
    pub dependency_count: u32,
    pub callers: Vec<AetherDependencyCaller>,
    pub dependencies: Vec<AetherDependencyTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDependencyCaller {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: Option<f32>,
    pub methods_called: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherDependencyTarget {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: Option<f32>,
    pub referencing_methods: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCallChainRequest {
    pub symbol_id: Option<String>,
    pub qualified_name: Option<String>,
    pub max_depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherCallChainResponse {
    pub found: bool,
    pub symbol_id: String,
    pub qualified_name: String,
    pub max_depth: u32,
    pub depth_count: u32,
    pub levels: Vec<Vec<AetherSymbolLookupMatch>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSearchRequest {
    pub query: String,
    pub limit: Option<u32>,
    pub mode: Option<SearchMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSearchResponse {
    pub query: String,
    pub limit: u32,
    pub mode_requested: SearchMode,
    pub mode_used: SearchMode,
    pub fallback_reason: Option<String>,
    pub result_count: u32,
    pub matches: Vec<AetherSymbolLookupMatch>,
}

impl AetherSymbolLookupResponse {
    fn from_search_envelope(
        query: String,
        limit: u32,
        envelope: SearchEnvelope<AetherSymbolLookupMatch>,
    ) -> Self {
        let SearchEnvelope {
            mode_requested,
            mode_used,
            fallback_reason,
            matches,
        } = envelope;
        let result_count = matches.len() as u32;

        Self {
            query,
            limit,
            mode_requested,
            mode_used,
            fallback_reason,
            result_count,
            matches,
        }
    }
}

impl AetherSearchResponse {
    fn from_search_envelope(
        query: String,
        limit: u32,
        envelope: SearchEnvelope<AetherSymbolLookupMatch>,
    ) -> Self {
        let SearchEnvelope {
            mode_requested,
            mode_used,
            fallback_reason,
            matches,
        } = envelope;
        let result_count = matches.len() as u32;

        Self {
            query,
            limit,
            mode_requested,
            mode_used,
            fallback_reason,
            result_count,
            matches,
        }
    }
}

impl From<SymbolSearchResult> for AetherSymbolLookupMatch {
    fn from(value: SymbolSearchResult) -> Self {
        Self {
            symbol_id: value.symbol_id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: None,
        }
    }
}

impl From<SymbolRecord> for AetherSymbolLookupMatch {
    fn from(value: SymbolRecord) -> Self {
        Self {
            symbol_id: value.id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: None,
        }
    }
}

impl From<SymbolRecord> for AetherDependencyCaller {
    fn from(value: SymbolRecord) -> Self {
        Self {
            symbol_id: value.id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: None,
            methods_called: None,
        }
    }
}

impl From<SymbolRecord> for AetherDependencyTarget {
    fn from(value: SymbolRecord) -> Self {
        Self {
            symbol_id: value.id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: None,
            referencing_methods: None,
        }
    }
}

impl AetherMcpServer {
    pub fn aether_symbol_lookup_logic(
        &self,
        request: AetherSymbolLookupRequest,
    ) -> Result<AetherSymbolLookupResponse, AetherMcpError> {
        let limit = effective_limit(request.limit);
        let matches = self.lexical_search_matches(&request.query, limit)?;
        let envelope = SearchEnvelope {
            mode_requested: SearchMode::Lexical,
            mode_used: SearchMode::Lexical,
            fallback_reason: None,
            matches,
        };

        Ok(AetherSymbolLookupResponse::from_search_envelope(
            request.query,
            limit,
            envelope,
        ))
    }

    pub async fn aether_dependencies_logic(
        &self,
        request: AetherDependenciesRequest,
    ) -> Result<AetherDependenciesResponse, AetherMcpError> {
        let symbol_id = request.symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(empty_dependencies_response(String::new()));
        }

        if !self.sqlite_path().exists() {
            return Ok(empty_dependencies_response(symbol_id.to_owned()));
        }

        let store = self.state.store.as_ref();
        let Some(symbol) = store.get_symbol_record(symbol_id)? else {
            return Ok(empty_dependencies_response(symbol_id.to_owned()));
        };

        let graph_store = self.state.graph.as_ref();
        if !is_type_symbol_kind(symbol.kind.as_str()) {
            let callers = graph_store
                .get_callers(&symbol.qualified_name)
                .await?
                .into_iter()
                .map(AetherDependencyCaller::from)
                .collect::<Vec<_>>();
            let dependencies = graph_store
                .get_dependencies(&symbol.id)
                .await?
                .into_iter()
                .map(AetherDependencyTarget::from)
                .collect::<Vec<_>>();

            return Ok(AetherDependenciesResponse {
                symbol_id: symbol_id.to_owned(),
                found: true,
                aggregated: false,
                child_method_count: 0,
                caller_count: callers.len() as u32,
                dependency_count: dependencies.len() as u32,
                callers,
                dependencies,
            });
        }

        let child_methods = child_method_symbols(store, &symbol)?;
        let child_method_count = child_methods.len() as u32;

        let mut callers_by_id = HashMap::<String, (SymbolRecord, HashSet<String>)>::new();
        let mut dependencies_by_id = HashMap::<String, (SymbolRecord, HashSet<String>)>::new();

        for child_method in &child_methods {
            let child_key = child_method.id.clone();

            for caller in graph_store
                .get_callers(child_method.qualified_name.as_str())
                .await?
            {
                let entry = callers_by_id
                    .entry(caller.id.clone())
                    .or_insert_with(|| (caller, HashSet::new()));
                entry.1.insert(child_key.clone());
            }

            for dependency in graph_store
                .get_dependencies(child_method.id.as_str())
                .await?
            {
                let entry = dependencies_by_id
                    .entry(dependency.id.clone())
                    .or_insert_with(|| (dependency, HashSet::new()));
                entry.1.insert(child_key.clone());
            }
        }

        let mut callers = callers_by_id
            .into_values()
            .map(|(record, child_methods)| AetherDependencyCaller {
                methods_called: Some(child_methods.len() as u32),
                ..AetherDependencyCaller::from(record)
            })
            .collect::<Vec<_>>();
        callers.sort_by(|left, right| {
            right
                .methods_called
                .cmp(&left.methods_called)
                .then_with(|| left.qualified_name.cmp(&right.qualified_name))
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        let mut dependencies = dependencies_by_id
            .into_values()
            .map(|(record, child_methods)| AetherDependencyTarget {
                referencing_methods: Some(child_methods.len() as u32),
                ..AetherDependencyTarget::from(record)
            })
            .collect::<Vec<_>>();
        dependencies.sort_by(|left, right| {
            right
                .referencing_methods
                .cmp(&left.referencing_methods)
                .then_with(|| left.qualified_name.cmp(&right.qualified_name))
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        Ok(AetherDependenciesResponse {
            symbol_id: symbol_id.to_owned(),
            found: true,
            aggregated: true,
            child_method_count,
            caller_count: callers.len() as u32,
            dependency_count: dependencies.len() as u32,
            callers,
            dependencies,
        })
    }

    pub async fn aether_call_chain_logic(
        &self,
        request: AetherCallChainRequest,
    ) -> Result<AetherCallChainResponse, AetherMcpError> {
        let symbol_id_input = request.symbol_id.as_deref().unwrap_or("").trim().to_owned();
        let qualified_name_input = request
            .qualified_name
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_owned();
        let max_depth = request.max_depth.unwrap_or(3).clamp(1, 10);

        if symbol_id_input.is_empty() && qualified_name_input.is_empty() {
            return Ok(AetherCallChainResponse {
                found: false,
                symbol_id: String::new(),
                qualified_name: String::new(),
                max_depth,
                depth_count: 0,
                levels: Vec::new(),
            });
        }

        if !self.sqlite_path().exists() {
            return Ok(AetherCallChainResponse {
                found: false,
                symbol_id: symbol_id_input,
                qualified_name: qualified_name_input,
                max_depth,
                depth_count: 0,
                levels: Vec::new(),
            });
        }

        let store = self.state.store.as_ref();
        let mut start_symbol = None;
        if !symbol_id_input.is_empty() {
            start_symbol = store.get_symbol_record(&symbol_id_input)?;
        }
        if start_symbol.is_none() && !qualified_name_input.is_empty() {
            start_symbol = store.get_symbol_by_qualified_name(&qualified_name_input)?;
        }
        let Some(start_symbol) = start_symbol else {
            return Ok(AetherCallChainResponse {
                found: false,
                symbol_id: symbol_id_input,
                qualified_name: qualified_name_input,
                max_depth,
                depth_count: 0,
                levels: Vec::new(),
            });
        };

        let graph_store = self.state.graph.as_ref();
        let levels = graph_store
            .get_call_chain(&start_symbol.id, max_depth)
            .await?
            .into_iter()
            .map(|rows| {
                rows.into_iter()
                    .map(AetherSymbolLookupMatch::from)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        Ok(AetherCallChainResponse {
            found: true,
            symbol_id: start_symbol.id,
            qualified_name: start_symbol.qualified_name,
            max_depth,
            depth_count: levels.len() as u32,
            levels,
        })
    }

    pub async fn aether_search_logic(
        &self,
        request: AetherSearchRequest,
    ) -> Result<AetherSearchResponse, AetherMcpError> {
        let mode_requested = request.mode.unwrap_or_default();
        let limit = effective_limit(request.limit);
        let search_config_ref = self.state.config.as_ref();
        let retrieval_limit = {
            let reranker = search_config_ref.search.reranker;
            let window = search_config_ref.search.rerank_window;
            if !matches!(reranker, SearchRerankerKind::None) {
                window.max(limit).clamp(1, 200)
            } else {
                limit
            }
        };
        let query = request.query.clone();
        let sqlite_path = self.sqlite_path();
        let store = Arc::clone(&self.state.store);
        let lexical = tokio::task::spawn_blocking(
            move || -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
                if !sqlite_path.exists() {
                    return Ok(Vec::new());
                }

                let matches = store.search_symbols(query.as_str(), retrieval_limit)?;
                Ok(matches
                    .into_iter()
                    .map(AetherSymbolLookupMatch::from)
                    .collect::<Vec<_>>())
            },
        )
        .await
        .map_err(|err| AetherMcpError::Message(format!("search join: {err}")))??;
        let reranker_kind = search_config_ref.search.reranker;
        let rerank_window = search_config_ref.search.rerank_window;
        let semantic_unavailability = if self.state.semantic_search_available {
            None
        } else {
            semantic_search_unavailability(search_config_ref)
        };

        let envelope = match mode_requested {
            SearchMode::Lexical => SearchEnvelope {
                mode_requested: SearchMode::Lexical,
                mode_used: SearchMode::Lexical,
                fallback_reason: None,
                matches: lexical,
            },
            SearchMode::Semantic => {
                if let Some(unavailability) = semantic_unavailability.as_ref() {
                    return Ok(AetherSearchResponse::from_search_envelope(
                        request.query,
                        limit,
                        SearchEnvelope {
                            mode_requested: SearchMode::Semantic,
                            mode_used: SearchMode::Lexical,
                            fallback_reason: Some(unavailability.fallback_reason()),
                            matches: lexical,
                        },
                    ));
                }

                let (semantic, fallback_reason) = self
                    .semantic_search_matches(&request.query, retrieval_limit)
                    .await?;
                if semantic.is_empty() {
                    SearchEnvelope {
                        mode_requested: SearchMode::Semantic,
                        mode_used: SearchMode::Lexical,
                        fallback_reason,
                        matches: lexical,
                    }
                } else {
                    SearchEnvelope {
                        mode_requested: SearchMode::Semantic,
                        mode_used: SearchMode::Semantic,
                        fallback_reason: None,
                        matches: semantic,
                    }
                }
            }
            SearchMode::Hybrid => {
                if let Some(unavailability) = semantic_unavailability.as_ref() {
                    return Ok(AetherSearchResponse::from_search_envelope(
                        request.query,
                        limit,
                        SearchEnvelope {
                            mode_requested: SearchMode::Hybrid,
                            mode_used: SearchMode::Lexical,
                            fallback_reason: Some(unavailability.fallback_reason()),
                            matches: lexical,
                        },
                    ));
                }

                let (semantic, fallback_reason) = self
                    .semantic_search_matches(&request.query, retrieval_limit)
                    .await?;
                if semantic.is_empty() {
                    SearchEnvelope {
                        mode_requested: SearchMode::Hybrid,
                        mode_used: SearchMode::Lexical,
                        fallback_reason,
                        matches: lexical,
                    }
                } else {
                    let fuse_limit = if matches!(reranker_kind, SearchRerankerKind::None) {
                        limit
                    } else {
                        rerank_window.max(limit).clamp(1, 200)
                    };
                    let fused = fuse_hybrid_matches(&lexical, &semantic, fuse_limit);
                    let matches = self
                        .maybe_rerank_hybrid_matches(
                            &request.query,
                            limit,
                            fused,
                            reranker_kind,
                            rerank_window,
                        )
                        .await?;
                    SearchEnvelope {
                        mode_requested: SearchMode::Hybrid,
                        mode_used: SearchMode::Hybrid,
                        fallback_reason: None,
                        matches,
                    }
                }
            }
        };

        Ok(AetherSearchResponse::from_search_envelope(
            request.query,
            limit,
            envelope,
        ))
    }

    fn lexical_search_matches(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        if !sqlite_path.exists() {
            return Ok(Vec::new());
        }

        let store = self.state.store.as_ref();
        let matches = store.search_symbols(query, limit)?;

        Ok(matches
            .into_iter()
            .map(AetherSymbolLookupMatch::from)
            .collect())
    }

    async fn semantic_search_matches(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<(Vec<AetherSymbolLookupMatch>, Option<String>), AetherMcpError> {
        let sqlite_path = self.sqlite_path();
        if !sqlite_path.exists() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED.to_owned()),
            ));
        }

        let store = self.state.store.as_ref();
        if store.list_all_symbol_ids()?.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
            ));
        }

        let loaded = load_embedding_provider_from_config(
            self.workspace(),
            EmbeddingProviderOverrides::default(),
        )?;
        let Some(loaded) = loaded else {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned()),
            ));
        };

        let query_embedding = match loaded
            .provider
            .embed_text_with_purpose(query, EmbeddingPurpose::Query)
            .await
        {
            Ok(embedding) => embedding,
            Err(err) => {
                return Ok((Vec::new(), Some(format!("embedding provider error: {err}"))));
            }
        };

        if query_embedding.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR.to_owned()),
            ));
        }

        let Some(vector_store) = self.state.vector_store.as_ref().map(Arc::clone) else {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned()),
            ));
        };
        let candidates = vector_store
            .search_nearest(
                &query_embedding,
                &loaded.provider_name,
                &loaded.model_name,
                limit,
            )
            .await?;
        if candidates.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
            ));
        }

        let mut matches = Vec::new();
        for candidate in candidates {
            let Some(symbol) = store.get_symbol_search_result(&candidate.symbol_id)? else {
                continue;
            };

            matches.push(AetherSymbolLookupMatch {
                symbol_id: symbol.symbol_id,
                qualified_name: symbol.qualified_name,
                file_path: symbol.file_path,
                language: symbol.language,
                kind: symbol.kind,
                semantic_score: Some(candidate.semantic_score),
            });
        }
        if matches.is_empty() {
            return Ok((
                Vec::new(),
                Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
            ));
        }

        Ok((matches, None))
    }

    async fn maybe_rerank_hybrid_matches(
        &self,
        query: &str,
        limit: u32,
        fused_matches: Vec<AetherSymbolLookupMatch>,
        reranker_kind: SearchRerankerKind,
        rerank_window: u32,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        if fused_matches.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 100) as usize;
        if matches!(reranker_kind, SearchRerankerKind::None) {
            return Ok(fused_matches.into_iter().take(limit).collect());
        }

        let fallback = fused_matches
            .iter()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let loaded = match load_reranker_provider_from_config(
            self.workspace(),
            RerankerProviderOverrides::default(),
        ) {
            Ok(Some(loaded)) => loaded,
            Ok(None) => return Ok(fallback),
            Err(err) => {
                tracing::warn!(error = %err, "reranker unavailable, falling back to RRF matches");
                return Ok(fallback);
            }
        };

        match self
            .rerank_matches_with_provider(
                query,
                limit,
                rerank_window,
                &fused_matches,
                loaded.provider.as_ref(),
            )
            .await
        {
            Ok(matches) => Ok(matches),
            Err(err) => {
                tracing::warn!(
                    provider = %loaded.provider_name,
                    error = %err,
                    "reranker failed, falling back to RRF matches"
                );
                Ok(fallback)
            }
        }
    }

    async fn rerank_matches_with_provider(
        &self,
        query: &str,
        limit: usize,
        rerank_window: u32,
        fused_matches: &[AetherSymbolLookupMatch],
        provider: &dyn RerankerProvider,
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        if fused_matches.is_empty() || query.trim().is_empty() || limit == 0 {
            return Ok(fused_matches.iter().take(limit).cloned().collect());
        }

        let window = rerank_window.max(limit as u32).clamp(1, 200) as usize;
        let candidate_matches = fused_matches
            .iter()
            .take(window.min(fused_matches.len()))
            .cloned()
            .collect::<Vec<_>>();

        let rerank_candidates = {
            let store = self.state.store.as_ref();
            let mut rerank_candidates = Vec::with_capacity(candidate_matches.len());
            for candidate in &candidate_matches {
                rerank_candidates.push(RerankCandidate {
                    id: candidate.symbol_id.clone(),
                    text: self.rerank_candidate_text(store, candidate)?,
                });
            }
            rerank_candidates
        };

        let reranked = provider.rerank(query, &rerank_candidates, limit).await?;

        let mut resolved = Vec::with_capacity(limit.min(candidate_matches.len()));
        let mut used = HashSet::new();

        for result in &reranked {
            if let Some(row) = candidate_matches.get(result.original_rank)
                && row.symbol_id == result.id
                && used.insert(row.symbol_id.clone())
            {
                resolved.push(row.clone());
                if resolved.len() >= limit {
                    break;
                }
                continue;
            }

            if let Some(row) = candidate_matches
                .iter()
                .find(|row| row.symbol_id == result.id && !used.contains(&row.symbol_id))
            {
                used.insert(row.symbol_id.clone());
                resolved.push(row.clone());
                if resolved.len() >= limit {
                    break;
                }
            }
        }

        for row in fused_matches {
            if resolved.len() >= limit {
                break;
            }
            if used.insert(row.symbol_id.clone()) {
                resolved.push(row.clone());
            }
        }

        Ok(resolved)
    }

    fn rerank_candidate_text(
        &self,
        store: &SqliteStore,
        row: &AetherSymbolLookupMatch,
    ) -> Result<String, AetherMcpError> {
        let fallback = format!(
            "qualified_name: {}\nkind: {}\nfile_path: {}",
            row.qualified_name, row.kind, row.file_path
        );

        let Some(blob) = store.read_sir_blob(&row.symbol_id)? else {
            return Ok(fallback);
        };

        let Ok(value) = serde_json::from_str::<Value>(&blob) else {
            return Ok(fallback);
        };

        let Some(intent) = value
            .get("intent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(fallback);
        };

        Ok(format!("{intent}\n{fallback}"))
    }
}

fn empty_dependencies_response(symbol_id: String) -> AetherDependenciesResponse {
    AetherDependenciesResponse {
        symbol_id,
        found: false,
        aggregated: false,
        child_method_count: 0,
        caller_count: 0,
        dependency_count: 0,
        callers: Vec::new(),
        dependencies: Vec::new(),
    }
}

fn fuse_hybrid_matches(
    lexical: &[AetherSymbolLookupMatch],
    semantic: &[AetherSymbolLookupMatch],
    limit: u32,
) -> Vec<AetherSymbolLookupMatch> {
    const RRF_K: f32 = 60.0;

    let mut by_id: HashMap<String, AetherSymbolLookupMatch> = HashMap::new();
    let mut score_by_id: HashMap<String, f32> = HashMap::new();

    for (rank, row) in lexical.iter().enumerate() {
        let id = row.symbol_id.clone();
        by_id.entry(id.clone()).or_insert_with(|| row.clone());
        *score_by_id.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }

    for (rank, row) in semantic.iter().enumerate() {
        let id = row.symbol_id.clone();
        by_id
            .entry(id.clone())
            .and_modify(|existing| {
                if existing.semantic_score.is_none() && row.semantic_score.is_some() {
                    existing.semantic_score = row.semantic_score;
                }
            })
            .or_insert_with(|| row.clone());
        *score_by_id.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }

    let mut ranked: Vec<(String, f32)> = score_by_id.into_iter().collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    ranked
        .into_iter()
        .take(limit.clamp(1, 100) as usize)
        .filter_map(|(symbol_id, _)| by_id.remove(&symbol_id))
        .collect()
}
