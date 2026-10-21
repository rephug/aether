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

use super::callers::CallerAttribution;
use super::{
    AetherMcpServer, LiveSymbolSources, ambiguous_selector, child_method_symbols, effective_limit,
    is_type_symbol_kind,
};
use crate::state::semantic_search_unavailability;
use crate::{AetherMcpError, SearchMode};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupRequest {
    /// Lexical query over symbol ids, qualified names, file paths, languages and kinds.
    /// Optional (empty by default) and ignored when `symbol_ids` is given.
    #[serde(default)]
    pub query: String,
    pub limit: Option<u32>,
    /// Exact symbol ids to return instead of a query's matches (up to 200), in the order
    /// given and without the lexical `limit`: a `/scan` batch fetches its targets' current
    /// source this way, so a file with more indexed symbols than the limit cannot hide any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_ids: Option<Vec<String>>,
    /// Also return each match's `source_text`, the exact text its `source_hash` was
    /// computed from (one read of the file), so a SIR reasoned from that text can be
    /// injected with that hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_source: Option<bool>,
}

/// Upper bound on `symbol_ids` per lookup call.
pub const SYMBOL_LOOKUP_MAX_IDS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSymbolLookupMatch {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: Option<f32>,
    /// The content hash of the symbol's source as the workspace file holds it now
    /// (`aether_symbol_lookup` only); `None` when the file cannot be read or parsed or no
    /// longer declares the symbol. Pass it as `source_hash` to `aether_sir_inject` so a
    /// SIR written for the text you read is refused if that text has since changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// The text `source_hash` was computed from, when `include_source` was requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_text: Option<String>,
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
    /// Whether the symbol is a type (struct, trait, enum, type alias, class or interface)
    /// whose callers and dependencies are rolled up: the callers of the type itself and of
    /// each of its methods, and what those methods call.
    pub aggregated: bool,
    /// How many methods the aggregated type declares, each overload counted; 0 when not
    /// aggregated.
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
    /// For an aggregated type, how many of its methods this caller calls, overloads of one
    /// name counted once. 0 when the caller only names the type itself, as a constructor
    /// call does.
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
    /// For an aggregated type, how many of its methods call this target, each overload
    /// counted.
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
        // Candidate retrieval can be wider than the requested result count
        // when reranking is enabled, including on a degraded lexical path.
        let mut matches = matches;
        matches.truncate(limit as usize);
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
            source_hash: None,
            source_text: None,
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
            source_hash: None,
            source_text: None,
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
        let mut matches = match request.symbol_ids.as_deref() {
            Some(ids) => self.symbol_matches_by_id(ids)?,
            None => self.lexical_search_matches(&request.query, limit)?,
        };
        // The hashes (and texts) come from the files as they are now, not from the
        // index, and both from one read per file, so a caller can reason over
        // `source_text` and bind the resulting `aether_sir_inject` to exactly that text.
        let include_source = request.include_source.unwrap_or(false);
        let mut live = LiveSymbolSources::new(&self.state.workspace);
        for entry in &mut matches {
            let Some(source) = live.source_for(&entry.file_path, &entry.symbol_id)? else {
                continue;
            };
            entry.source_hash = Some(source.source_hash.clone());
            if include_source {
                entry.source_text = Some(source.source_text.clone());
            }
        }
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
        let mut attribution = CallerAttribution::new(store, self.workspace());
        if !is_type_symbol_kind(symbol.kind.as_str()) {
            let callers = attribution
                .callers_of(&symbol)?
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

        // A call that names the type itself, such as a constructor call (`new Order()` in
        // Java or TypeScript, `Point(1, 2)` for a Rust tuple struct), makes its caller a
        // caller of the type; only calls to methods add to `methods_called`.
        let mut callers_by_id = attribution
            .callers_of(&symbol)?
            .into_iter()
            .map(|caller| (caller.id.clone(), (caller, HashSet::<String>::new())))
            .collect::<HashMap<_, _>>();
        let mut dependencies_by_id = HashMap::<String, (SymbolRecord, HashSet<String>)>::new();

        for child_method in &child_methods {
            let child_key = child_method.id.clone();

            // A call to an overloaded name is attributed to every overload, so callers
            // count the methods they call by name.
            for caller in attribution.callers_of(child_method)? {
                let entry = callers_by_id
                    .entry(caller.id.clone())
                    .or_insert_with(|| (caller, HashSet::new()));
                entry.1.insert(child_method.qualified_name.clone());
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
            let exact_matches =
                store.find_symbol_search_results_by_qualified_name(&qualified_name_input)?;
            match exact_matches.as_slice() {
                [] => {}
                [only] => start_symbol = store.get_symbol_record(only.symbol_id.as_str())?,
                many => return Err(ambiguous_selector(qualified_name_input.as_str(), many)),
            }
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

                let terms = query
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let matches = store.search_symbols_by_terms(&terms, retrieval_limit)?;
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
                    let (matches, rerank_fallback_reason) = self
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
                        fallback_reason: rerank_fallback_reason,
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

    /// The indexed symbols with exactly these ids, in the order given; ids the index does
    /// not hold are left out. Not subject to the lexical `limit`.
    fn symbol_matches_by_id(
        &self,
        ids: &[String],
    ) -> Result<Vec<AetherSymbolLookupMatch>, AetherMcpError> {
        if ids.len() > SYMBOL_LOOKUP_MAX_IDS {
            return Err(AetherMcpError::Message(format!(
                "symbol_ids holds {} ids; at most {SYMBOL_LOOKUP_MAX_IDS} per call",
                ids.len()
            )));
        }
        if !self.sqlite_path().exists() {
            return Ok(Vec::new());
        }
        let store = self.state.store.as_ref();
        let mut matches = Vec::with_capacity(ids.len());
        for id in ids {
            let id = id.trim();
            if id.is_empty() {
                continue;
            }
            if let Some(record) = store.get_symbol_record(id)? {
                matches.push(AetherSymbolLookupMatch::from(record));
            }
        }
        Ok(matches)
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
        if self.state.read_only
            && self.state.config.embeddings.enabled
            && self.state.vector_store.is_none()
        {
            // Opening Lance through the writable migration path is not a read.
            // Refuse before provider initialization or a query embedding request.
            return Ok((
                Vec::new(),
                Some("configured vector backend is unavailable in read-only mode".to_owned()),
            ));
        }
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
                source_hash: None,
                source_text: None,
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
    ) -> Result<(Vec<AetherSymbolLookupMatch>, Option<String>), AetherMcpError> {
        if fused_matches.is_empty() {
            return Ok((Vec::new(), None));
        }

        let limit = limit.clamp(1, 100) as usize;
        if matches!(reranker_kind, SearchRerankerKind::None) {
            return Ok((fused_matches.into_iter().take(limit).collect(), None));
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
            Ok(None) => {
                return Ok((
                    fallback,
                    Some("configured reranker is unavailable; using reciprocal-rank fusion without reranking".to_owned()),
                ));
            }
            Err(err) => {
                tracing::warn!(error = %err, "reranker unavailable, falling back to RRF matches");
                return Ok((
                    fallback,
                    Some(format!(
                        "reranker unavailable: {err}; using reciprocal-rank fusion without reranking"
                    )),
                ));
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
            Ok(matches) => Ok((matches, None)),
            Err(err) => {
                tracing::warn!(
                    provider = %loaded.provider_name,
                    error = %err,
                    "reranker failed, falling back to RRF matches"
                );
                Ok((
                    fallback,
                    Some(format!(
                        "reranker {} failed: {err}; using reciprocal-rank fusion without reranking",
                        loaded.provider_name
                    )),
                ))
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
        .take(limit.clamp(1, 200) as usize)
        .filter_map(|(symbol_id, _)| by_id.remove(&symbol_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use aether_config::{AetherConfig, GraphBackend, save_workspace_config};
    use aether_infer::MockRerankerProvider;
    use tempfile::tempdir;

    use super::*;

    fn seed_symbol(store: &SqliteStore, id: &str, name: &str, path: &str) -> anyhow::Result<()> {
        store.upsert_symbol(SymbolRecord {
            id: id.to_owned(),
            qualified_name: name.to_owned(),
            file_path: path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            signature_fingerprint: "signature".to_owned(),
            last_seen_at: 1,
        })?;
        Ok(())
    }

    fn test_config(workspace: &std::path::Path) -> anyhow::Result<AetherConfig> {
        let mut config = AetherConfig::default();
        config.embeddings.enabled = false;
        config.storage.graph_backend = GraphBackend::Sqlite;
        save_workspace_config(workspace, &config)?;
        Ok(config)
    }

    #[tokio::test]
    async fn readonly_unavailable_vectors_return_before_loading_a_provider() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let mut config = test_config(temp.path())?;
        config.embeddings.enabled = true;
        config.embeddings.vector_backend = aether_config::EmbeddingVectorBackend::Lancedb;
        save_workspace_config(temp.path(), &config)?;
        // Create only the catalog. A read-only open must not initialize Lance.
        let store = SqliteStore::open(temp.path())?;
        seed_symbol(&store, "candidate", "Auth::login", "src/auth.rs")?;
        let server = AetherMcpServer::from_state(
            Arc::new(crate::SharedState::open_readonly(temp.path())?),
            false,
        );
        let (matches, reason) = server.semantic_search_matches("auth", 1).await?;
        assert!(matches.is_empty());
        assert_eq!(
            reason.as_deref(),
            Some("configured vector backend is unavailable in read-only mode")
        );
        Ok(())
    }

    #[tokio::test]
    async fn ranked_lexical_search_finds_joint_terms_and_honors_result_limit() -> anyhow::Result<()>
    {
        let temp = tempdir()?;
        let mut config = test_config(temp.path())?;
        config.search.reranker = SearchRerankerKind::Cohere;
        config.search.rerank_window = 200;
        save_workspace_config(temp.path(), &config)?;
        let server = AetherMcpServer::new(temp.path(), false)?;
        for index in 0..40 {
            seed_symbol(
                &server.state.store,
                &format!("auth-{index:02}"),
                &format!("Alpha{index:02}::login"),
                "src/auth.rs",
            )?;
            seed_symbol(
                &server.state.store,
                &format!("cache-{index:02}"),
                &format!("Beta{index:02}::refresh"),
                "src/cache.rs",
            )?;
        }
        seed_symbol(
            &server.state.store,
            "both",
            "Zulu::auth_cache",
            "src/auth_cache.rs",
        )?;
        seed_symbol(
            &server.state.store,
            "auth-cache-noise",
            "Aardvark::other",
            "src/other.rs",
        )?;
        let response = server
            .aether_search_logic(AetherSearchRequest {
                query: "auth cache".to_owned(),
                limit: Some(2),
                mode: Some(SearchMode::Lexical),
            })
            .await?;
        assert_eq!(response.result_count, 2);
        assert_eq!(response.matches[0].symbol_id, "both");
        assert!(
            response
                .matches
                .iter()
                .all(|row| row.symbol_id != "auth-cache-noise")
        );
        assert_eq!(response.fallback_reason, None);
        Ok(())
    }

    #[tokio::test]
    async fn reranker_can_select_candidate_beyond_the_first_hundred() -> anyhow::Result<()> {
        let temp = tempdir()?;
        test_config(temp.path())?;
        let server = AetherMcpServer::new(temp.path(), false)?;
        for index in 0..150 {
            seed_symbol(
                &server.state.store,
                &format!("candidate-{index:03}"),
                &format!("Candidate{index:03}::target"),
                "src/lib.rs",
            )?;
        }
        let lexical = server
            .state
            .store
            .search_symbols_by_terms(&["target".to_owned()], 200)?
            .into_iter()
            .map(AetherSymbolLookupMatch::from)
            .collect::<Vec<_>>();
        assert_eq!(lexical.len(), 150);
        let fused = fuse_hybrid_matches(&lexical, &[], 200);
        assert_eq!(fused.len(), 150);
        let provider =
            MockRerankerProvider::new(HashMap::from([("candidate-149".to_owned(), 1.0)]));
        let ranked = server
            .rerank_matches_with_provider("target", 1, 200, &fused, &provider)
            .await?;
        assert_eq!(ranked[0].symbol_id, "candidate-149");
        Ok(())
    }

    #[tokio::test]
    async fn unavailable_reranker_reports_degraded_fusion_results() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let mut config = test_config(temp.path())?;
        config.search.reranker = SearchRerankerKind::Cohere;
        // A unique, absent key name makes the loader fail before any request.
        config.providers.cohere.api_key_env = format!(
            "AETHER_TEST_UNSET_RERANKER_KEY_{}_{}",
            std::process::id(),
            temp.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        );
        save_workspace_config(temp.path(), &config)?;
        let server = AetherMcpServer::new(temp.path(), false)?;
        seed_symbol(
            &server.state.store,
            "candidate",
            "Auth::login",
            "src/auth.rs",
        )?;
        let lexical = server.lexical_search_matches("Auth", 1)?;
        let (matches, fallback_reason) = server
            .maybe_rerank_hybrid_matches("auth", 1, lexical, SearchRerankerKind::Cohere, 200)
            .await?;
        assert_eq!(matches.len(), 1);
        let reason = fallback_reason.as_deref().unwrap_or_default();
        assert!(reason.contains("reranker unavailable"));
        assert!(reason.contains("without reranking"));
        let response = AetherSearchResponse::from_search_envelope(
            "auth".to_owned(),
            1,
            SearchEnvelope {
                mode_requested: SearchMode::Hybrid,
                mode_used: SearchMode::Hybrid,
                fallback_reason: fallback_reason.clone(),
                matches,
            },
        );
        assert_eq!(response.fallback_reason, fallback_reason);
        Ok(())
    }
}
