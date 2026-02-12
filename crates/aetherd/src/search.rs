use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use aether_core::{
    SEARCH_FALLBACK_EMBEDDING_EMPTY_QUERY_VECTOR, SEARCH_FALLBACK_EMBEDDINGS_DISABLED,
    SEARCH_FALLBACK_LOCAL_STORE_NOT_INITIALIZED, SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY,
    SearchEnvelope,
};
use aether_infer::{EmbeddingProviderOverrides, load_embedding_provider_from_config};
use aether_store::{SemanticSearchResult, SqliteStore, Store, SymbolSearchResult};
use anyhow::{Context, Result};
use serde_json::json;

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
    let limit = limit.clamp(1, 100);

    let lexical_matches = lexical_search(&store, query, limit)?;
    match mode {
        SearchMode::Lexical => Ok(SearchExecution {
            mode_requested: SearchMode::Lexical,
            mode_used: SearchMode::Lexical,
            fallback_reason: None,
            matches: lexical_matches,
        }),
        SearchMode::Semantic => {
            let (semantic_matches, fallback_reason) =
                semantic_search(workspace, &store, query, limit, store_present)?;
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
            let (semantic_matches, fallback_reason) =
                semantic_search(workspace, &store, query, limit, store_present)?;
            if semantic_matches.is_empty() {
                return Ok(SearchExecution {
                    mode_requested: SearchMode::Hybrid,
                    mode_used: SearchMode::Lexical,
                    fallback_reason,
                    matches: lexical_matches,
                });
            }

            Ok(SearchExecution {
                mode_requested: SearchMode::Hybrid,
                mode_used: SearchMode::Hybrid,
                fallback_reason: None,
                matches: fuse_hybrid_results(&lexical_matches, &semantic_matches, limit),
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
    limit: u32,
    store_present: bool,
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

    let matches = store
        .search_symbols_semantic(
            &query_embedding,
            &loaded.provider_name,
            &loaded.model_name,
            limit,
        )
        .context("failed to run semantic symbol search")?;
    if matches.is_empty() {
        return Ok((
            Vec::new(),
            Some(SEARCH_FALLBACK_SEMANTIC_INDEX_NOT_READY.to_owned()),
        ));
    }

    Ok((
        matches.into_iter().map(SearchResultRow::from).collect(),
        None,
    ))
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

impl From<SemanticSearchResult> for SearchResultRow {
    fn from(value: SemanticSearchResult) -> Self {
        Self {
            symbol_id: value.symbol_id,
            qualified_name: value.qualified_name,
            file_path: value.file_path,
            language: value.language,
            kind: value.kind,
            semantic_score: Some(value.semantic_score),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use aether_infer::{EmbeddingProvider, InferenceProvider, MockEmbeddingProvider, MockProvider};
    use tempfile::tempdir;

    use super::*;
    use crate::observer::ObserverState;
    use crate::sir_pipeline::SirPipeline;
    use aether_store::SymbolRecord;

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
        write_embeddings_enabled_config(workspace);

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

    fn write_embeddings_enabled_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[inference]
provider = "mock"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true

[embeddings]
enabled = true
provider = "mock"
"#,
        )
        .expect("write config");
    }
}
