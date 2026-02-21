use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use aether_config::{
    AetherConfig, SearchCalibratedThresholdsConfig, SearchRerankerKind, SearchThresholdsConfig,
    ensure_workspace_config,
};
use aether_core::{
    SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR, SEARCH_FALLBACK_EMBEDDINGS_DISABLED,
    SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED, SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY,
    SearchEnvelope,
};
use aether_infer::{
    EmbeddingProviderOverrides, RerankCandidate, RerankerProvider, RerankerProviderOverrides,
    load_embedding_provider_from_config, load_reranker_provider_from_config,
};
use aether_store::{
    SqliteStore, Store, SymbolSearchResult, ThresholdCalibrationRecord, open_vector_store,
};
use anyhow::{Context, Result};
use serde_json::{Value, json};

pub use aether_core::SearchMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchOutputFormat {
    #[default]
    Table,
    Json,
}

impl SearchOutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Json => "json",
        }
    }
}

impl std::str::FromStr for SearchOutputFormat {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "invalid output format '{other}', expected one of: table, json"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResultRow {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: Option<f32>,
}

pub type SearchExecution = SearchEnvelope<SearchResultRow>;

pub fn run_search_once(
    workspace: &Path,
    query: &str,
    limit: u32,
    mode: SearchMode,
    output_format: SearchOutputFormat,
    out: &mut dyn Write,
) -> Result<()> {
    let execution = execute_search(workspace, query, limit, mode)?;
    if let Some(reason) = &execution.fallback_reason {
        tracing::warn!(fallback_reason = %reason, "AETHER search fallback");
    }

    match output_format {
        SearchOutputFormat::Table => write_search_results(&execution.matches, out)
            .context("failed to write search results")?,
        SearchOutputFormat::Json => write_search_results_json(&execution, out)
            .context("failed to write search JSON output")?,
    }
    Ok(())
}

pub fn execute_search(
    workspace: &Path,
    query: &str,
    limit: u32,
    mode: SearchMode,
) -> Result<SearchExecution> {
    let store_present = workspace.join(".aether").join("meta.sqlite").exists();
    let store = SqliteStore::open(workspace).context("failed to initialize local store")?;
    let config =
        ensure_workspace_config(workspace).context("failed to load workspace config for search")?;
    let limit = limit.clamp(1, 100);
    let (normalized_query, language_hint) = extract_language_hint_from_query(query);
    let search_config = config.search.clone();
    let retrieval_limit = if matches!(mode, SearchMode::Hybrid)
        && !matches!(search_config.reranker, SearchRerankerKind::None)
    {
        search_config.rerank_window.max(limit).clamp(1, 200)
    } else {
        limit
    };

    let lexical_matches = lexical_search(&store, &normalized_query, retrieval_limit)?;
    match mode {
        SearchMode::Lexical => Ok(SearchExecution {
            mode_requested: SearchMode::Lexical,
            mode_used: SearchMode::Lexical,
            fallback_reason: None,
            matches: lexical_matches,
        }),
        SearchMode::Semantic => {
            let (semantic_matches, fallback_reason) = semantic_search(
                workspace,
                &store,
                &normalized_query,
                language_hint.as_deref(),
                limit,
                store_present,
                &config,
            )?;
            if semantic_matches.is_empty() {
                return Ok(SearchExecution {
                    mode_requested: SearchMode::Semantic,
                    mode_used: SearchMode::Lexical,
                    fallback_reason,
                    matches: lexical_matches,
                });
            }

            Ok(SearchExecution {
                mode_requested: SearchMode::Semantic,
                mode_used: SearchMode::Semantic,
                fallback_reason: None,
                matches: semantic_matches,
            })
        }
        SearchMode::Hybrid => {
            let (semantic_matches, fallback_reason) = semantic_search(
                workspace,
                &store,
                &normalized_query,
                language_hint.as_deref(),
                retrieval_limit,
                store_present,
                &config,
            )?;
            if semantic_matches.is_empty() {
                return Ok(SearchExecution {
                    mode_requested: SearchMode::Hybrid,
                    mode_used: SearchMode::Lexical,
                    fallback_reason,
                    matches: lexical_matches,
                });
            }

            let fuse_limit = retrieval_limit;
            let fused = fuse_hybrid_results(&lexical_matches, &semantic_matches, fuse_limit);
            let matches = maybe_rerank_hybrid_results(
                workspace,
                &store,
                &normalized_query,
                limit,
                fused,
                search_config.reranker,
                search_config.rerank_window,
            )?;

            Ok(SearchExecution {
                mode_requested: SearchMode::Hybrid,
                mode_used: SearchMode::Hybrid,
                fallback_reason: None,
                matches,
            })
        }
    }
}

pub fn write_search_results(
    matches: &[SearchResultRow],
    out: &mut dyn Write,
) -> std::io::Result<()> {
    writeln!(out, "symbol_id\tqualified_name\tfile_path\tlanguage\tkind")?;

    for entry in matches {
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            normalize_search_field(&entry.symbol_id),
            normalize_search_field(&entry.qualified_name),
            normalize_search_field(&entry.file_path),
            normalize_search_field(&entry.language),
            normalize_search_field(&entry.kind)
        )?;
    }

    Ok(())
}

pub fn write_search_results_json(
    execution: &SearchExecution,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    let matches = execution
        .matches
        .iter()
        .map(|entry| {
            json!({
                "symbol_id": &entry.symbol_id,
                "qualified_name": &entry.qualified_name,
                "file_path": &entry.file_path,
                "language": &entry.language,
                "kind": &entry.kind,
                "semantic_score": entry.semantic_score
            })
        })
        .collect::<Vec<_>>();

    let payload = json!({
        "mode_requested": execution.mode_requested.as_str(),
        "mode_used": execution.mode_used.as_str(),
        "fallback_reason": execution.fallback_reason.as_deref(),
        "matches": matches,
    });

    serde_json::to_writer(&mut *out, &payload).map_err(std::io::Error::other)?;
    writeln!(out)?;
    Ok(())
}

fn lexical_search(store: &SqliteStore, query: &str, limit: u32) -> Result<Vec<SearchResultRow>> {
    let matches = store
        .search_symbols(query, limit)
        .context("failed to search symbols lexically")?;

    Ok(matches.into_iter().map(SearchResultRow::from).collect())
}

fn semantic_search(
    workspace: &Path,
    store: &SqliteStore,
    query: &str,
    query_language_hint: Option<&str>,
    limit: u32,
    store_present: bool,
    config: &AetherConfig,
) -> Result<(Vec<SearchResultRow>, Option<String>)> {
    if !store_present {
        return Ok((
            Vec::new(),
            Some(SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED.to_owned()),
        ));
    }

    let loaded =
        load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())
            .context("failed to load embedding provider")?;
    let Some(loaded) = loaded else {
        return Ok((
            Vec::new(),
            Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED.to_owned()),
        ));
    };

    let calibration_by_language = store
        .list_threshold_calibrations()
        .context("failed to load threshold calibration metadata")?
        .into_iter()
        .map(|record| {
            (
                normalize_threshold_language(&record.language).to_owned(),
                record,
            )
        })
        .collect::<HashMap<_, _>>();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for semantic search")?;
    let query_embedding = match runtime.block_on(loaded.provider.embed_text(query)) {
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

    let vector_store = runtime
        .block_on(open_vector_store(workspace))
        .context("failed to open vector store")?;
    let matches = runtime
        .block_on(vector_store.search_nearest(
            &query_embedding,
            &loaded.provider_name,
            &loaded.model_name,
            limit,
        ))
        .context("failed to run semantic symbol search")?;
    if matches.is_empty() {
        return Ok((
            Vec::new(),
            Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
        ));
    }

    let mut semantic_rows = Vec::new();
    let mut mismatched_languages = HashSet::new();
    let symbol_ids = matches
        .iter()
        .map(|candidate| candidate.symbol_id.clone())
        .collect::<Vec<_>>();
    let symbols_by_id = store
        .get_symbol_search_results_batch(symbol_ids.as_slice())
        .context("failed to resolve semantic search symbols in batch")?;
    let mut threshold_context = ThresholdResolutionContext {
        query_language_hint,
        thresholds: &config.search.thresholds,
        calibrated_thresholds: &config.search.calibrated_thresholds,
        calibration_by_language: &calibration_by_language,
        current_provider: &loaded.provider_name,
        current_model: &loaded.model_name,
        mismatched_languages: &mut mismatched_languages,
    };
    for candidate in matches {
        let Some(symbol) = symbols_by_id.get(candidate.symbol_id.as_str()) else {
            continue;
        };

        let threshold = resolve_effective_threshold(&symbol.language, &mut threshold_context);
        if candidate.semantic_score < threshold {
            continue;
        }

        semantic_rows.push(SearchResultRow {
            symbol_id: symbol.symbol_id.clone(),
            qualified_name: symbol.qualified_name.clone(),
            file_path: symbol.file_path.clone(),
            language: symbol.language.clone(),
            kind: symbol.kind.clone(),
            semantic_score: Some(candidate.semantic_score),
        });
    }

    if !mismatched_languages.is_empty() {
        let mut languages = mismatched_languages.into_iter().collect::<Vec<_>>();
        languages.sort();

        for language in languages {
            if let Some(calibrated) = calibration_by_language.get(language.as_str()) {
                tracing::warn!(
                    language = %language,
                    calibrated_provider = %calibrated.provider,
                    calibrated_model = %calibrated.model,
                    current_provider = %loaded.provider_name,
                    current_model = %loaded.model_name,
                    "embedding provider/model differs from threshold calibration metadata; using defaults"
                );
            }
        }
    }

    if semantic_rows.is_empty() {
        return Ok((
            Vec::new(),
            Some("semantic matches below configured similarity thresholds".to_owned()),
        ));
    }

    Ok((semantic_rows, None))
}

fn extract_language_hint_from_query(query: &str) -> (String, Option<String>) {
    let mut hint = None;
    let mut retained_tokens = Vec::new();

    for token in query.split_whitespace() {
        if hint.is_none() {
            let lower = token.to_ascii_lowercase();
            let maybe_hint = lower
                .strip_prefix("lang:")
                .or_else(|| lower.strip_prefix("language:"))
                .map(|value| {
                    value
                        .trim_matches(|ch: char| {
                            !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                        })
                        .to_owned()
                });

            if let Some(value) = maybe_hint
                && !value.is_empty()
            {
                hint = Some(normalize_threshold_language(&value).to_owned());
                continue;
            }
        }

        retained_tokens.push(token);
    }

    (retained_tokens.join(" ").trim().to_owned(), hint)
}

struct ThresholdResolutionContext<'a> {
    query_language_hint: Option<&'a str>,
    thresholds: &'a SearchThresholdsConfig,
    calibrated_thresholds: &'a SearchCalibratedThresholdsConfig,
    calibration_by_language: &'a HashMap<String, ThresholdCalibrationRecord>,
    current_provider: &'a str,
    current_model: &'a str,
    mismatched_languages: &'a mut HashSet<String>,
}

fn resolve_effective_threshold(
    candidate_language: &str,
    context: &mut ThresholdResolutionContext<'_>,
) -> f32 {
    let target_language = context.query_language_hint.unwrap_or(candidate_language);
    let normalized = normalize_threshold_language(target_language);

    if context
        .thresholds
        .is_manual_override_for_language(normalized)
    {
        return context.thresholds.value_for_language(normalized);
    }

    if let Some(calibration) = context.calibration_by_language.get(normalized) {
        if calibration.provider.trim() == context.current_provider
            && calibration.model.trim() == context.current_model
        {
            if let Some(configured) = context.calibrated_thresholds.value_for_language(normalized) {
                return configured;
            }
            return calibration.threshold;
        }

        context.mismatched_languages.insert(normalized.to_owned());
    }

    context.thresholds.value_for_language(normalized)
}

fn normalize_threshold_language(language: &str) -> &'static str {
    let normalized = language.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "rust" | "rs" => "rust",
        "typescript" | "ts" | "tsx" | "javascript" | "js" => "typescript",
        "python" | "py" => "python",
        _ => "default",
    }
}

fn maybe_rerank_hybrid_results(
    workspace: &Path,
    store: &SqliteStore,
    query: &str,
    limit: u32,
    fused_results: Vec<SearchResultRow>,
    reranker_kind: SearchRerankerKind,
    rerank_window: u32,
) -> Result<Vec<SearchResultRow>> {
    if fused_results.is_empty() {
        return Ok(Vec::new());
    }

    if matches!(reranker_kind, SearchRerankerKind::None) {
        return Ok(fused_results
            .into_iter()
            .take(limit.clamp(1, 100) as usize)
            .collect());
    }

    let loaded =
        match load_reranker_provider_from_config(workspace, RerankerProviderOverrides::default())
            .context("failed to load reranker provider")
        {
            Ok(Some(loaded)) => loaded,
            Ok(None) => {
                return Ok(fused_results
                    .into_iter()
                    .take(limit.clamp(1, 100) as usize)
                    .collect());
            }
            Err(err) => {
                tracing::warn!(error = %err, "reranker unavailable, falling back to RRF results");
                return Ok(fused_results
                    .into_iter()
                    .take(limit.clamp(1, 100) as usize)
                    .collect());
            }
        };

    match rerank_rows_with_provider(
        store,
        query,
        &fused_results,
        limit,
        rerank_window,
        loaded.provider.as_ref(),
    ) {
        Ok(rows) => Ok(rows),
        Err(err) => {
            tracing::warn!(
                provider = %loaded.provider_name,
                error = %err,
                "reranker failed, falling back to RRF results"
            );
            Ok(fused_results
                .into_iter()
                .take(limit.clamp(1, 100) as usize)
                .collect())
        }
    }
}

fn rerank_rows_with_provider(
    store: &SqliteStore,
    query: &str,
    fused_results: &[SearchResultRow],
    limit: u32,
    rerank_window: u32,
    provider: &dyn RerankerProvider,
) -> Result<Vec<SearchResultRow>> {
    let limit = limit.clamp(1, 100) as usize;
    if fused_results.is_empty() || limit == 0 || query.trim().is_empty() {
        return Ok(fused_results.iter().take(limit).cloned().collect());
    }

    let window = rerank_window.max(limit as u32).clamp(1, 200) as usize;
    let candidate_rows = fused_results
        .iter()
        .take(window.min(fused_results.len()))
        .cloned()
        .collect::<Vec<_>>();

    if candidate_rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut rerank_candidates = Vec::with_capacity(candidate_rows.len());
    for row in &candidate_rows {
        rerank_candidates.push(RerankCandidate {
            id: row.symbol_id.clone(),
            text: rerank_candidate_text(store, row)?,
        });
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build runtime for reranker")?;
    let reranked = runtime
        .block_on(provider.rerank(query, &rerank_candidates, limit))
        .context("reranker request failed")?;

    let mut resolved = Vec::with_capacity(limit.min(candidate_rows.len()));
    let mut used = HashSet::new();

    for result in &reranked {
        if let Some(row) = candidate_rows.get(result.original_rank)
            && row.symbol_id == result.id
            && used.insert(row.symbol_id.clone())
        {
            resolved.push(row.clone());
            if resolved.len() >= limit {
                break;
            }
            continue;
        }

        if let Some(row) = candidate_rows
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

    for row in fused_results {
        if resolved.len() >= limit {
            break;
        }
        if used.insert(row.symbol_id.clone()) {
            resolved.push(row.clone());
        }
    }

    Ok(resolved)
}

fn rerank_candidate_text(store: &SqliteStore, row: &SearchResultRow) -> Result<String> {
    let fallback = format!(
        "qualified_name: {}\nkind: {}\nfile_path: {}",
        row.qualified_name, row.kind, row.file_path
    );

    let Some(blob) = store
        .read_sir_blob(&row.symbol_id)
        .with_context(|| format!("failed to load SIR blob for {}", row.symbol_id))?
    else {
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

fn fuse_hybrid_results(
    lexical: &[SearchResultRow],
    semantic: &[SearchResultRow],
    limit: u32,
) -> Vec<SearchResultRow> {
    const RRF_K: f32 = 60.0;

    let mut row_by_id: HashMap<String, SearchResultRow> = HashMap::new();
    let mut score_by_id: HashMap<String, f32> = HashMap::new();

    for (rank, row) in lexical.iter().enumerate() {
        let id = row.symbol_id.clone();
        row_by_id.entry(id.clone()).or_insert_with(|| row.clone());
        *score_by_id.entry(id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }

    for (rank, row) in semantic.iter().enumerate() {
        let id = row.symbol_id.clone();
        row_by_id
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
        .take(limit as usize)
        .filter_map(|(symbol_id, _)| row_by_id.remove(&symbol_id))
        .collect()
}

fn normalize_search_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

impl From<SymbolSearchResult> for SearchResultRow {
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

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::sync::Arc;

    use aether_infer::{
        EmbeddingProvider, InferenceProvider, MockEmbeddingProvider, MockProvider,
        MockRerankerProvider,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::observer::ObserverState;
    use crate::sir_pipeline::SirPipeline;
    use aether_store::{
        SymbolEmbeddingRecord, SymbolRecord, ThresholdCalibrationRecord, open_vector_store,
    };

    #[test]
    fn write_search_results_outputs_stable_header_and_columns() {
        let mut out = Vec::new();
        let matches = vec![SearchResultRow {
            symbol_id: "sym-1".to_owned(),
            qualified_name: "demo::run".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            semantic_score: None,
        }];

        write_search_results(&matches, &mut out).expect("write output");
        let rendered = String::from_utf8(out).expect("utf8 output");
        let lines: Vec<&str> = rendered.lines().collect();

        assert_eq!(
            lines[0],
            "symbol_id\tqualified_name\tfile_path\tlanguage\tkind"
        );
        assert_eq!(lines.len(), 2);

        let columns: Vec<&str> = lines[1].split('\t').collect();
        assert_eq!(columns.len(), 5);
        assert_eq!(columns[1], "demo::run");
    }

    #[test]
    fn run_search_once_reads_symbols_from_store() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        store
            .upsert_symbol(SymbolRecord {
                id: "sym-1".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::alpha".to_owned(),
                signature_fingerprint: "sig-a".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");

        let mut out = Vec::new();
        run_search_once(
            workspace,
            "alpha",
            20,
            SearchMode::Lexical,
            SearchOutputFormat::Table,
            &mut out,
        )
        .expect("run search");

        let rendered = String::from_utf8(out).expect("utf8 output");
        assert!(rendered.contains("symbol_id\tqualified_name\tfile_path\tlanguage\tkind"));
        assert!(rendered.contains("demo::alpha"));
    }

    #[test]
    fn write_search_results_json_outputs_stable_shape() {
        let mut out = Vec::new();
        let execution = SearchExecution {
            mode_requested: SearchMode::Hybrid,
            mode_used: SearchMode::Lexical,
            fallback_reason: Some("embeddings are disabled in .aether/config.toml".to_owned()),
            matches: vec![SearchResultRow {
                symbol_id: "sym-1".to_owned(),
                qualified_name: "demo::run".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: None,
            }],
        };

        write_search_results_json(&execution, &mut out).expect("json output");
        let rendered = String::from_utf8(out).expect("utf8 output");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("valid json output");

        assert_eq!(value["mode_requested"], "hybrid");
        assert_eq!(value["mode_used"], "lexical");
        assert_eq!(
            value["fallback_reason"],
            "embeddings are disabled in .aether/config.toml"
        );
        assert_eq!(value["matches"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["matches"][0]["symbol_id"], "sym-1");
    }

    #[test]
    fn semantic_search_returns_expected_top_match_with_mock_embeddings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_enabled_config_with_thresholds(
            workspace,
            "sqlite",
            SearchThresholdsConfig {
                default: 0.65,
                rust: 0.50,
                typescript: 0.65,
                python: 0.60,
            },
        );

        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-auth".to_owned(),
                file_path: "src/auth.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::auth_token_refresh".to_owned(),
                signature_fingerprint: "sig-auth".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert auth symbol");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-cache".to_owned(),
                file_path: "src/cache.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::cache_lookup".to_owned(),
                signature_fingerprint: "sig-cache".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert cache symbol");

        let provider = MockEmbeddingProvider;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let auth_embedding = runtime
            .block_on(provider.embed_text("oauth token refresh session auth"))
            .expect("auth embedding");
        let cache_embedding = runtime
            .block_on(provider.embed_text("cache lookup memory hit"))
            .expect("cache embedding");

        store
            .upsert_symbol_embedding(aether_store::SymbolEmbeddingRecord {
                symbol_id: "sym-auth".to_owned(),
                sir_hash: "hash-auth".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: auth_embedding,
                updated_at: 1_700_000_100,
            })
            .expect("upsert auth embedding");
        store
            .upsert_symbol_embedding(aether_store::SymbolEmbeddingRecord {
                symbol_id: "sym-cache".to_owned(),
                sir_hash: "hash-cache".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: cache_embedding,
                updated_at: 1_700_000_100,
            })
            .expect("upsert cache embedding");

        let semantic = execute_search(workspace, "oauth refresh token", 10, SearchMode::Semantic)
            .expect("semantic execution");
        assert_eq!(semantic.mode_used, SearchMode::Semantic);
        assert!(semantic.fallback_reason.is_none());
        assert!(!semantic.matches.is_empty());
        assert_eq!(semantic.matches[0].symbol_id, "sym-auth");

        let hybrid = execute_search(workspace, "oauth refresh token", 10, SearchMode::Hybrid)
            .expect("hybrid execution");
        assert_eq!(hybrid.mode_used, SearchMode::Hybrid);
        assert!(!hybrid.matches.is_empty());
        assert_eq!(hybrid.matches[0].symbol_id, "sym-auth");
    }

    #[test]
    fn semantic_search_respects_per_language_thresholds() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_enabled_config_with_thresholds(
            workspace,
            "sqlite",
            SearchThresholdsConfig {
                default: 0.65,
                rust: 0.95,
                typescript: 0.65,
                python: 0.40,
            },
        );

        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-rust".to_owned(),
                file_path: "src/auth.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::auth_token_refresh".to_owned(),
                signature_fingerprint: "sig-rust".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert rust symbol");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-python".to_owned(),
                file_path: "src/jobs.py".to_owned(),
                language: "python".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "jobs.refresh_token".to_owned(),
                signature_fingerprint: "sig-python".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert python symbol");

        let provider = MockEmbeddingProvider;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let rust_embedding = runtime
            .block_on(provider.embed_text("oauth token refresh session auth"))
            .expect("rust embedding");
        let python_embedding = runtime
            .block_on(provider.embed_text("oauth refresh token"))
            .expect("python embedding");

        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-rust".to_owned(),
                sir_hash: "hash-rust".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: rust_embedding,
                updated_at: 1_700_000_100,
            })
            .expect("upsert rust embedding");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-python".to_owned(),
                sir_hash: "hash-python".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: python_embedding,
                updated_at: 1_700_000_101,
            })
            .expect("upsert python embedding");

        let result = execute_search(workspace, "oauth refresh token", 10, SearchMode::Semantic)
            .expect("semantic search");
        assert_eq!(result.mode_used, SearchMode::Semantic);
        assert!(!result.matches.is_empty());
        assert!(
            result
                .matches
                .iter()
                .all(|row| row.language == "python" || row.language == "default")
        );
        assert!(
            result
                .matches
                .iter()
                .any(|row| row.symbol_id == "sym-python")
        );
    }

    #[test]
    fn threshold_precedence_manual_overrides_calibrated_and_default() {
        let mut mismatched = HashSet::new();
        let mut thresholds = SearchThresholdsConfig::default();
        thresholds.rust = 0.83;
        let calibrated_thresholds = SearchCalibratedThresholdsConfig {
            rust: Some(0.61),
            ..SearchCalibratedThresholdsConfig::default()
        };

        let mut calibration = HashMap::new();
        calibration.insert(
            "rust".to_owned(),
            ThresholdCalibrationRecord {
                language: "rust".to_owned(),
                threshold: 0.61,
                sample_size: 100,
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                calibrated_at: "1700000000".to_owned(),
            },
        );

        let mut context = ThresholdResolutionContext {
            query_language_hint: None,
            thresholds: &thresholds,
            calibrated_thresholds: &calibrated_thresholds,
            calibration_by_language: &calibration,
            current_provider: "mock",
            current_model: "mock-64d",
            mismatched_languages: &mut mismatched,
        };

        let resolved = resolve_effective_threshold("rust", &mut context);
        assert_eq!(resolved, 0.83);
        assert!(mismatched.is_empty());
    }

    #[test]
    fn threshold_precedence_uses_calibrated_before_default_when_no_manual_override() {
        let mut mismatched = HashSet::new();
        let thresholds = SearchThresholdsConfig::default();
        let calibrated_thresholds = SearchCalibratedThresholdsConfig {
            rust: Some(0.62),
            ..SearchCalibratedThresholdsConfig::default()
        };

        let mut calibration = HashMap::new();
        calibration.insert(
            "rust".to_owned(),
            ThresholdCalibrationRecord {
                language: "rust".to_owned(),
                threshold: 0.64,
                sample_size: 100,
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                calibrated_at: "1700000000".to_owned(),
            },
        );

        let mut context = ThresholdResolutionContext {
            query_language_hint: None,
            thresholds: &thresholds,
            calibrated_thresholds: &calibrated_thresholds,
            calibration_by_language: &calibration,
            current_provider: "mock",
            current_model: "mock-64d",
            mismatched_languages: &mut mismatched,
        };

        let resolved = resolve_effective_threshold("rust", &mut context);
        assert_eq!(resolved, 0.62);
        assert!(mismatched.is_empty());
    }

    #[test]
    fn provider_mismatch_uses_default_and_marks_warning() {
        let mut mismatched = HashSet::new();
        let thresholds = SearchThresholdsConfig::default();
        let calibrated_thresholds = SearchCalibratedThresholdsConfig {
            rust: Some(0.62),
            ..SearchCalibratedThresholdsConfig::default()
        };

        let mut calibration = HashMap::new();
        calibration.insert(
            "rust".to_owned(),
            ThresholdCalibrationRecord {
                language: "rust".to_owned(),
                threshold: 0.62,
                sample_size: 100,
                provider: "qwen3_local".to_owned(),
                model: "qwen3-embeddings-0.6B".to_owned(),
                calibrated_at: "1700000000".to_owned(),
            },
        );

        let mut context = ThresholdResolutionContext {
            query_language_hint: None,
            thresholds: &thresholds,
            calibrated_thresholds: &calibrated_thresholds,
            calibration_by_language: &calibration,
            current_provider: "mock",
            current_model: "mock-64d",
            mismatched_languages: &mut mismatched,
        };

        let resolved = resolve_effective_threshold("rust", &mut context);
        assert_eq!(resolved, thresholds.rust);
        assert!(mismatched.contains("rust"));
    }

    #[test]
    fn extract_language_hint_from_query_strips_hint_tokens() {
        let (query, hint) = extract_language_hint_from_query("lang:rust oauth refresh token");
        assert_eq!(query, "oauth refresh token");
        assert_eq!(hint.as_deref(), Some("rust"));

        let (query, hint) =
            extract_language_hint_from_query("find symbols language:python token handling");
        assert_eq!(query, "find symbols token handling");
        assert_eq!(hint.as_deref(), Some("python"));
    }

    #[test]
    fn hybrid_pipeline_with_reranker_none_matches_rrf_output() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");
        let lexical = vec![
            SearchResultRow {
                symbol_id: "sym-a".to_owned(),
                qualified_name: "demo::alpha".to_owned(),
                file_path: "src/a.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: None,
            },
            SearchResultRow {
                symbol_id: "sym-b".to_owned(),
                qualified_name: "demo::beta".to_owned(),
                file_path: "src/b.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: None,
            },
        ];
        let semantic = vec![
            SearchResultRow {
                symbol_id: "sym-b".to_owned(),
                qualified_name: "demo::beta".to_owned(),
                file_path: "src/b.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: Some(0.9),
            },
            SearchResultRow {
                symbol_id: "sym-c".to_owned(),
                qualified_name: "demo::gamma".to_owned(),
                file_path: "src/c.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: Some(0.6),
            },
        ];

        let fused = fuse_hybrid_results(&lexical, &semantic, 10);
        let without_reranker = maybe_rerank_hybrid_results(
            workspace,
            &store,
            "alpha beta",
            10,
            fused.clone(),
            SearchRerankerKind::None,
            50,
        )
        .expect("reranker none branch");

        assert_eq!(without_reranker, fused);
    }

    #[test]
    fn mock_reranker_reorders_fused_candidates() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");
        store
            .write_sir_blob("sym-a", r#"{"intent":"auth refresh token flow"}"#)
            .expect("write sir blob a");
        store
            .write_sir_blob("sym-b", r#"{"intent":"cache lookup"}"#)
            .expect("write sir blob b");
        store
            .write_sir_blob("sym-c", r#"{"intent":"http middleware"}"#)
            .expect("write sir blob c");

        let fused = vec![
            SearchResultRow {
                symbol_id: "sym-a".to_owned(),
                qualified_name: "demo::alpha".to_owned(),
                file_path: "src/a.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: Some(0.4),
            },
            SearchResultRow {
                symbol_id: "sym-b".to_owned(),
                qualified_name: "demo::beta".to_owned(),
                file_path: "src/b.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: Some(0.3),
            },
            SearchResultRow {
                symbol_id: "sym-c".to_owned(),
                qualified_name: "demo::gamma".to_owned(),
                file_path: "src/c.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                semantic_score: Some(0.2),
            },
        ];

        let provider = MockRerankerProvider::new(HashMap::from([
            ("sym-b".to_owned(), 0.95),
            ("sym-a".to_owned(), 0.60),
            ("sym-c".to_owned(), 0.10),
        ]));
        let reranked = rerank_rows_with_provider(&store, "token refresh", &fused, 3, 50, &provider)
            .expect("rerank rows with mock provider");

        assert_eq!(reranked[0].symbol_id, "sym-b");
        assert_eq!(reranked[1].symbol_id, "sym-a");
        assert_eq!(reranked[2].symbol_id, "sym-c");
    }

    #[test]
    fn lancedb_vector_store_round_trip_with_mock_embeddings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_enabled_config_with_backend(workspace, "lancedb");

        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-auth".to_owned(),
                file_path: "src/auth.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::auth_token_refresh".to_owned(),
                signature_fingerprint: "sig-auth".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert auth symbol");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-cache".to_owned(),
                file_path: "src/cache.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::cache_lookup".to_owned(),
                signature_fingerprint: "sig-cache".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert cache symbol");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let vector_store = runtime
            .block_on(open_vector_store(workspace))
            .expect("open lancedb vector store");

        runtime
            .block_on(vector_store.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-auth".to_owned(),
                sir_hash: "hash-auth".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![1.0, 0.0],
                updated_at: 1_700_000_200,
            }))
            .expect("upsert auth embedding");
        runtime
            .block_on(vector_store.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-cache".to_owned(),
                sir_hash: "hash-cache".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![0.0, 1.0],
                updated_at: 1_700_000_201,
            }))
            .expect("upsert cache embedding");

        let matches = runtime
            .block_on(vector_store.search_nearest(&[1.0, 0.0], "mock", "mock-64d", 10))
            .expect("search nearest");
        assert!(!matches.is_empty());
        assert_eq!(matches[0].symbol_id, "sym-auth");

        let meta = runtime
            .block_on(vector_store.get_embedding_meta("sym-auth"))
            .expect("meta lookup")
            .expect("meta exists");
        assert_eq!(meta.embedding_dim, 2);
        assert_eq!(meta.provider, "mock");
        assert_eq!(meta.model, "mock-64d");
    }

    #[test]
    fn lancedb_search_returns_expected_top_k_ordering() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_enabled_config_with_backend(workspace, "lancedb");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let vector_store = runtime
            .block_on(open_vector_store(workspace))
            .expect("open lancedb vector store");

        for (symbol_id, embedding) in [
            ("sym-1", vec![1.0, 0.0]),
            ("sym-2", vec![0.9, 0.1]),
            ("sym-3", vec![0.0, 1.0]),
        ] {
            runtime
                .block_on(vector_store.upsert_embedding(SymbolEmbeddingRecord {
                    symbol_id: symbol_id.to_owned(),
                    sir_hash: format!("hash-{symbol_id}"),
                    provider: "mock".to_owned(),
                    model: "mock-64d".to_owned(),
                    embedding,
                    updated_at: 1_700_001_000,
                }))
                .expect("upsert embedding");
        }

        let matches = runtime
            .block_on(vector_store.search_nearest(&[1.0, 0.0], "mock", "mock-64d", 2))
            .expect("search nearest");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].symbol_id, "sym-1");
        assert_eq!(matches[1].symbol_id, "sym-2");
    }

    #[test]
    fn lancedb_migrates_existing_sqlite_embeddings() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_enabled_config_with_backend(workspace, "lancedb");

        let store = SqliteStore::open(workspace).expect("open sqlite store");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-1".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::one".to_owned(),
                signature_fingerprint: "sig-1".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert first symbol");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-2".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::two".to_owned(),
                signature_fingerprint: "sig-2".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert second symbol");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-1".to_owned(),
                sir_hash: "hash-1".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![1.0, 0.0],
                updated_at: 1_700_000_100,
            })
            .expect("upsert sqlite embedding one");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-2".to_owned(),
                sir_hash: "hash-2".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![0.0, 1.0],
                updated_at: 1_700_000_100,
            })
            .expect("upsert sqlite embedding two");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let vector_store = runtime
            .block_on(open_vector_store(workspace))
            .expect("open lancedb vector store");

        let matches = runtime
            .block_on(vector_store.search_nearest(&[1.0, 0.0], "mock", "mock-64d", 10))
            .expect("search migrated embeddings");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].symbol_id, "sym-1");

        let first = runtime
            .block_on(vector_store.get_embedding_meta("sym-1"))
            .expect("get migrated meta")
            .expect("migrated meta exists");
        let second = runtime
            .block_on(vector_store.get_embedding_meta("sym-2"))
            .expect("get migrated meta")
            .expect("migrated meta exists");
        assert_eq!(first.sir_hash, "hash-1");
        assert_eq!(second.sir_hash, "hash-2");
    }

    #[test]
    fn vector_backend_toggle_between_sqlite_and_lancedb() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_enabled_config_with_backend(workspace, "sqlite");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let sqlite_backend = runtime
            .block_on(open_vector_store(workspace))
            .expect("open sqlite vector store");
        SqliteStore::open(workspace)
            .expect("open sqlite store")
            .upsert_symbol(SymbolRecord {
                id: "sym-toggle".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::toggle".to_owned(),
                signature_fingerprint: "sig-toggle".to_owned(),
                last_seen_at: 1_700_000_300,
            })
            .expect("upsert toggle symbol");
        runtime
            .block_on(sqlite_backend.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-toggle".to_owned(),
                sir_hash: "hash-toggle".to_owned(),
                provider: "mock".to_owned(),
                model: "mock-64d".to_owned(),
                embedding: vec![1.0, 0.0],
                updated_at: 1_700_000_300,
            }))
            .expect("upsert sqlite backend");
        let sqlite_matches = runtime
            .block_on(sqlite_backend.search_nearest(&[1.0, 0.0], "mock", "mock-64d", 5))
            .expect("sqlite search");
        assert_eq!(sqlite_matches[0].symbol_id, "sym-toggle");

        write_embeddings_enabled_config_with_backend(workspace, "lancedb");
        let lance_backend = runtime
            .block_on(open_vector_store(workspace))
            .expect("open lancedb vector store");
        let lance_matches = runtime
            .block_on(lance_backend.search_nearest(&[1.0, 0.0], "mock", "mock-64d", 5))
            .expect("lancedb search");
        assert_eq!(lance_matches[0].symbol_id, "sym-toggle");
    }

    #[test]
    fn semantic_search_falls_back_to_lexical_when_embeddings_disabled() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "sym-1".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "demo::fallback_alpha".to_owned(),
                signature_fingerprint: "sig-a".to_owned(),
                last_seen_at: 1_700_000_000,
            })
            .expect("upsert symbol");

        let result = execute_search(workspace, "fallback_alpha", 10, SearchMode::Semantic)
            .expect("semantic with fallback");
        assert_eq!(result.mode_requested, SearchMode::Semantic);
        assert_eq!(result.mode_used, SearchMode::Lexical);
        assert!(result.fallback_reason.is_some());
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].symbol_id, "sym-1");
    }

    #[test]
    fn semantic_search_falls_back_when_local_store_not_initialized() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        let result = execute_search(workspace, "anything", 10, SearchMode::Semantic)
            .expect("semantic with no store");
        assert_eq!(result.mode_requested, SearchMode::Semantic);
        assert_eq!(result.mode_used, SearchMode::Lexical);
        assert_eq!(
            result.fallback_reason.as_deref(),
            Some("local store not initialized")
        );
        assert!(result.matches.is_empty());
    }

    #[test]
    fn search_reflects_symbol_rename_and_removal() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();

        fs::create_dir_all(workspace.join("src")).expect("create src dir");
        let rust_file = workspace.join("src/lib.rs");
        fs::write(
            &rust_file,
            "fn alpha() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
        )
        .expect("write source");

        let mut observer = ObserverState::new(workspace.to_path_buf()).expect("observer");
        observer.seed_from_disk().expect("seed observer");

        let store = SqliteStore::open(workspace).expect("open store");
        let provider: Arc<dyn InferenceProvider> = Arc::new(MockProvider);
        let pipeline =
            SirPipeline::new_with_provider(workspace.to_path_buf(), 2, provider, "mock", "mock")
                .expect("pipeline");

        let mut startup_stdout = Vec::new();
        for event in observer.initial_symbol_events() {
            pipeline
                .process_event(&store, &event, false, &mut startup_stdout)
                .expect("process startup event");
        }

        let alpha_hits = store.search_symbols("alpha", 20).expect("search alpha");
        assert_eq!(alpha_hits.len(), 1);

        fs::write(
            &rust_file,
            "fn gamma() -> i32 { 1 }\nfn beta() -> i32 { 2 }\n",
        )
        .expect("write renamed source");
        let rename_event = observer
            .process_path(&rust_file)
            .expect("process rename path")
            .expect("expected rename event");
        let mut update_stdout = Vec::new();
        pipeline
            .process_event(&store, &rename_event, false, &mut update_stdout)
            .expect("process rename event");

        let alpha_after_rename = store
            .search_symbols("alpha", 20)
            .expect("search alpha after rename");
        let gamma_after_rename = store
            .search_symbols("gamma", 20)
            .expect("search gamma after rename");
        assert!(alpha_after_rename.is_empty());
        assert_eq!(gamma_after_rename.len(), 1);

        fs::write(&rust_file, "fn gamma() -> i32 { 1 }\n").expect("write removal source");
        let removal_event = observer
            .process_path(&rust_file)
            .expect("process removal path")
            .expect("expected removal event");
        let mut removal_stdout = Vec::new();
        pipeline
            .process_event(&store, &removal_event, false, &mut removal_stdout)
            .expect("process removal event");

        let beta_after_remove = store
            .search_symbols("beta", 20)
            .expect("search beta after remove");
        assert!(beta_after_remove.is_empty());
    }

    fn write_embeddings_enabled_config_with_backend(workspace: &Path, vector_backend: &str) {
        write_embeddings_enabled_config_with_thresholds(
            workspace,
            vector_backend,
            SearchThresholdsConfig::default(),
        );
    }

    fn write_embeddings_enabled_config_with_thresholds(
        workspace: &Path,
        vector_backend: &str,
        thresholds: SearchThresholdsConfig,
    ) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            format!(
                r#"[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = true
provider = "mock"
vector_backend = "{vector_backend}"

[search.thresholds]
default = {}
rust = {}
typescript = {}
python = {}
"#,
                thresholds.default, thresholds.rust, thresholds.typescript, thresholds.python
            ),
        )
        .expect("write config");
    }
}
