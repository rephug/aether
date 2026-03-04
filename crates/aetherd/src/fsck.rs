use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use aether_config::{EmbeddingVectorBackend, GraphBackend, load_workspace_config};
use aether_core::{Language, Position, SourceRange, Symbol, SymbolKind, content_hash};
use aether_infer::ProviderOverrides;
use aether_sir::SirAnnotation;
use aether_store::{
    IntentOperation, LanceVectorStore, SqliteStore, Store, SurrealGraphStore, WriteIntent,
    WriteIntentStatus, open_vector_store,
};
use anyhow::{Context, Result};

use crate::sir_pipeline::{DEFAULT_SIR_CONCURRENCY, SirPipeline};

#[derive(Debug, Clone, Default)]
pub struct FsckReport {
    pub symbols_in_sqlite: usize,
    pub vectors_in_store: usize,
    pub graph_nodes_in_surreal: usize,
    pub symbols_missing_vectors: usize,
    pub symbols_missing_graph_nodes: usize,
    pub phantom_graph_nodes: usize,
    pub dangling_edges: usize,
    pub orphaned_vectors: usize,
    pub incomplete_write_intents: usize,
    pub replayed_incomplete_intents: usize,
    pub queued_reembedding: usize,
    pub repaired_missing_graph_nodes: usize,
    pub removed_phantom_graph_nodes: usize,
    pub removed_dangling_edges: usize,
    pub removed_orphaned_vectors: usize,
    pub vector_check_skipped: bool,
    pub graph_check_error: Option<String>,
}

pub fn run_fsck(workspace: &Path, repair: bool, verbose: bool) -> Result<FsckReport> {
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("failed to resolve workspace {}", workspace.display()))?;
    let config = load_workspace_config(&workspace).context("failed to load workspace config")?;
    let store = SqliteStore::open_readonly(&workspace).context("failed to open SQLite store")?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for fsck")?;

    let sqlite_symbol_ids = store
        .list_all_symbol_ids()
        .context("failed to list SQLite symbol IDs")?;
    let sqlite_symbol_ids_set = sqlite_symbol_ids
        .iter()
        .map(|value| value.as_str())
        .collect::<HashSet<_>>();
    let sqlite_sir_symbol_ids = store
        .list_symbol_ids_with_sir()
        .context("failed to list SQLite symbol IDs with SIR")?;

    let mut report = FsckReport {
        symbols_in_sqlite: sqlite_symbol_ids.len(),
        ..FsckReport::default()
    };

    let mut vector_ids = Vec::new();
    let mut missing_vector_ids = Vec::new();
    match config.embeddings.vector_backend {
        EmbeddingVectorBackend::Lancedb => {
            let lance = runtime
                .block_on(LanceVectorStore::open(&workspace))
                .context("failed to open LanceDB store")?;
            let has_tables = runtime
                .block_on(lance.has_embedding_tables())
                .context("failed to inspect LanceDB tables")?;
            if has_tables {
                let mut existing = HashSet::new();
                for chunk in sqlite_sir_symbol_ids.chunks(500) {
                    let chunk_ids = chunk.to_vec();
                    let found = runtime
                        .block_on(lance.list_existing_embedding_symbol_ids(chunk_ids.as_slice()))
                        .context("failed to query LanceDB symbol embedding presence")?;
                    existing.extend(found);
                }
                missing_vector_ids = sqlite_sir_symbol_ids
                    .iter()
                    .filter(|symbol_id| !existing.contains(symbol_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                vector_ids = runtime
                    .block_on(lance.list_all_embedding_symbol_ids())
                    .context("failed to list LanceDB vector IDs")?;
            } else {
                report.vector_check_skipped = true;
                if verbose {
                    eprintln!("fsck: LanceDB has no embedding tables; skipping vector checks");
                }
            }
        }
        EmbeddingVectorBackend::Sqlite => {
            vector_ids = store
                .list_all_embedding_symbol_ids()
                .context("failed to list SQLite embedding IDs")?;
            let vector_ids_set = vector_ids
                .iter()
                .map(|value| value.as_str())
                .collect::<HashSet<_>>();
            missing_vector_ids = sqlite_sir_symbol_ids
                .iter()
                .filter(|symbol_id| !vector_ids_set.contains(symbol_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
        }
    }
    let orphaned_vector_ids = vector_ids
        .iter()
        .filter(|symbol_id| !sqlite_symbol_ids_set.contains(symbol_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();

    report.vectors_in_store = vector_ids.len();
    report.symbols_missing_vectors = missing_vector_ids.len();
    report.orphaned_vectors = orphaned_vector_ids.len();

    let mut missing_graph_node_ids = Vec::new();
    let mut phantom_graph_node_ids = Vec::new();
    let mut dangling_edges = Vec::new();
    let mut surreal_graph_opt = None::<SurrealGraphStore>;
    match config.storage.graph_backend {
        GraphBackend::Surreal => match runtime.block_on(SurrealGraphStore::open(&workspace)) {
            Ok(graph) => {
                let mut existing_for_sqlite = HashSet::new();
                for chunk in sqlite_symbol_ids.chunks(500) {
                    let chunk_ids = chunk.to_vec();
                    let found = runtime
                        .block_on(graph.list_existing_symbol_ids(chunk_ids.as_slice()))
                        .context("failed to query Surreal symbol node presence")?;
                    existing_for_sqlite.extend(found);
                }

                missing_graph_node_ids = sqlite_symbol_ids
                    .iter()
                    .filter(|symbol_id| !existing_for_sqlite.contains(symbol_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();

                let graph_symbol_ids = runtime
                    .block_on(graph.list_all_symbol_ids())
                    .context("failed to list Surreal graph symbol IDs")?;
                report.graph_nodes_in_surreal = graph_symbol_ids.len();

                phantom_graph_node_ids = graph_symbol_ids
                    .iter()
                    .filter(|symbol_id| !sqlite_symbol_ids_set.contains(symbol_id.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();

                let edges = runtime
                    .block_on(graph.list_dependency_edges())
                    .context("failed to list Surreal dependency edges")?;
                dangling_edges = edges
                    .into_iter()
                    .filter(|edge| {
                        !sqlite_symbol_ids_set.contains(edge.source_symbol_id.as_str())
                            || !sqlite_symbol_ids_set.contains(edge.target_symbol_id.as_str())
                    })
                    .collect::<Vec<_>>();

                surreal_graph_opt = Some(graph);
            }
            Err(err) => {
                report.graph_check_error =
                    Some(format!("failed to open Surreal graph store: {err}"));
            }
        },
        GraphBackend::Sqlite | GraphBackend::Cozo => {
            report.graph_check_error = Some(format!(
                "graph backend '{}' does not expose Surreal fsck checks; skipped",
                config.storage.graph_backend.as_str()
            ));
        }
    }

    report.symbols_missing_graph_nodes = missing_graph_node_ids.len();
    report.phantom_graph_nodes = phantom_graph_node_ids.len();
    report.dangling_edges = dangling_edges.len();

    let incomplete_intents = store
        .get_incomplete_intents()
        .context("failed to query incomplete write intents")?;
    report.incomplete_write_intents = incomplete_intents.len();

    if repair {
        let repair_store =
            SqliteStore::open(&workspace).context("failed to open writable SQLite store")?;
        let sir_pipeline = SirPipeline::new(
            workspace.clone(),
            DEFAULT_SIR_CONCURRENCY,
            ProviderOverrides::default(),
        )
        .context("failed to initialize sir pipeline for fsck repairs")?;
        report.replayed_incomplete_intents = sir_pipeline
            .replay_incomplete_intents(&repair_store, true, 100, verbose)
            .context("failed to replay incomplete write intents during repair")?;

        if !missing_vector_ids.is_empty() {
            for symbol_id in &missing_vector_ids {
                let Some(symbol_record) = repair_store
                    .get_symbol_record(symbol_id.as_str())
                    .with_context(|| format!("failed to load symbol record {symbol_id}"))?
                else {
                    continue;
                };
                let Some(sir_blob) = repair_store
                    .read_sir_blob(symbol_id.as_str())
                    .with_context(|| format!("failed to load SIR blob for {symbol_id}"))?
                else {
                    continue;
                };
                let sir = match serde_json::from_str::<SirAnnotation>(sir_blob.as_str()) {
                    Ok(sir) => sir,
                    Err(err) => {
                        if verbose {
                            eprintln!(
                                "fsck: skipping re-embed queue for {} due invalid SIR JSON: {}",
                                symbol_id, err
                            );
                        }
                        continue;
                    }
                };
                let payload_json = serde_json::to_string(&serde_json::json!({
                    "symbol": synthesize_symbol(&symbol_record),
                    "sir": sir,
                    "provider_name": "repair",
                    "model_name": "repair",
                    "commit_hash": null
                }))
                .with_context(|| format!("failed to serialize repair payload for {symbol_id}"))?;
                let intent_id = content_hash(
                    format!("fsck-reembed\n{}\n{}", symbol_id, unix_timestamp_millis()).as_str(),
                );
                let intent = WriteIntent {
                    intent_id,
                    symbol_id: symbol_id.clone(),
                    file_path: symbol_record.file_path.clone(),
                    operation: IntentOperation::UpsertSir,
                    status: WriteIntentStatus::SqliteDone,
                    payload_json: Some(payload_json),
                    created_at: unix_timestamp_secs(),
                    completed_at: None,
                    error_message: None,
                };
                repair_store
                    .create_write_intent(&intent)
                    .with_context(|| format!("failed to queue re-embed intent for {symbol_id}"))?;
                report.queued_reembedding += 1;
            }
        }

        if let Some(graph) = surreal_graph_opt.as_ref() {
            if !missing_graph_node_ids.is_empty() {
                let mut file_paths = BTreeSet::new();
                for symbol_id in &missing_graph_node_ids {
                    if let Some(record) = repair_store
                        .get_symbol_record(symbol_id.as_str())
                        .with_context(|| format!("failed to resolve symbol {symbol_id}"))?
                    {
                        file_paths.insert(record.file_path);
                    }
                }
                for file_path in file_paths {
                    runtime
                        .block_on(repair_store.sync_graph_for_file(graph, file_path.as_str()))
                        .with_context(|| format!("failed to repair graph for file {file_path}"))?;
                }
                report.repaired_missing_graph_nodes = missing_graph_node_ids.len();
            }

            for symbol_id in &phantom_graph_node_ids {
                runtime
                    .block_on(graph.delete_symbol_by_symbol_id(symbol_id.as_str()))
                    .with_context(|| format!("failed to delete phantom graph node {symbol_id}"))?;
                report.removed_phantom_graph_nodes += 1;
            }

            let mut removed_pairs = HashSet::new();
            for edge in &dangling_edges {
                let key = (edge.source_symbol_id.clone(), edge.target_symbol_id.clone());
                if !removed_pairs.insert(key.clone()) {
                    continue;
                }
                runtime
                    .block_on(graph.delete_dependency_edges_by_pair(key.0.as_str(), key.1.as_str()))
                    .with_context(|| {
                        format!(
                            "failed to delete dangling edge {} -> {}",
                            key.0.as_str(),
                            key.1.as_str()
                        )
                    })?;
                report.removed_dangling_edges += 1;
            }
        }

        if !orphaned_vector_ids.is_empty() && !report.vector_check_skipped {
            let vector_store = runtime
                .block_on(open_vector_store(&workspace))
                .context("failed to open vector store for repairs")?;
            for symbol_id in &orphaned_vector_ids {
                runtime
                    .block_on(vector_store.delete_embedding(symbol_id.as_str()))
                    .with_context(|| format!("failed to delete orphaned vector {symbol_id}"))?;
                report.removed_orphaned_vectors += 1;
            }
        }
    }

    print_report(&report, repair);
    Ok(report)
}

fn print_report(report: &FsckReport, repair: bool) {
    println!("AETHER State Verification Report");
    println!("=================================");
    println!("Symbols in SQLite:        {}", report.symbols_in_sqlite);
    println!("Vectors in LanceDB:       {}", report.vectors_in_store);
    println!(
        "Graph nodes in SurrealDB: {}",
        report.graph_nodes_in_surreal
    );
    println!();
    println!("Inconsistencies found:");
    println!(
        "  Symbols missing vectors:      {}{}",
        report.symbols_missing_vectors,
        if repair && report.queued_reembedding > 0 {
            " (queued for repair)"
        } else {
            ""
        }
    );
    println!(
        "  Symbols missing graph nodes:  {}{}",
        report.symbols_missing_graph_nodes,
        if repair && report.repaired_missing_graph_nodes > 0 {
            " (repaired)"
        } else {
            ""
        }
    );
    println!(
        "  Phantom graph nodes:          {}",
        report.phantom_graph_nodes
    );
    println!(
        "  Dangling edges:               {}{}",
        report.dangling_edges,
        if repair && report.removed_dangling_edges > 0 {
            " (removed)"
        } else {
            ""
        }
    );
    println!(
        "  Orphaned vectors:             {}{}",
        report.orphaned_vectors,
        if repair && report.removed_orphaned_vectors > 0 {
            " (removed)"
        } else {
            ""
        }
    );
    println!(
        "  Incomplete write intents:     {}{}",
        report.incomplete_write_intents,
        if repair && report.replayed_incomplete_intents > 0 {
            " (replayed)"
        } else {
            ""
        }
    );
    if report.vector_check_skipped {
        println!();
        println!("Vector checks skipped: no LanceDB embedding tables found.");
    }
    if let Some(error) = report.graph_check_error.as_deref() {
        println!();
        println!("Graph check note: {error}");
    }
    if repair {
        println!();
        println!("Repair complete. Run `aether fsck` again to verify.");
    }
}

fn synthesize_symbol(record: &aether_store::SymbolRecord) -> Symbol {
    Symbol {
        id: record.id.clone(),
        language: parse_language(record.language.as_str()),
        file_path: record.file_path.clone(),
        kind: parse_kind(record.kind.as_str()),
        name: record
            .qualified_name
            .rsplit("::")
            .next()
            .unwrap_or(record.qualified_name.as_str())
            .to_owned(),
        qualified_name: record.qualified_name.clone(),
        signature_fingerprint: record.signature_fingerprint.clone(),
        content_hash: content_hash(
            format!(
                "{}\n{}\n{}",
                record.id, record.qualified_name, record.signature_fingerprint
            )
            .as_str(),
        ),
        range: SourceRange {
            start: Position { line: 1, column: 1 },
            end: Position { line: 1, column: 1 },
            start_byte: None,
            end_byte: None,
        },
    }
}

fn parse_language(raw: &str) -> Language {
    match raw.trim().to_ascii_lowercase().as_str() {
        "rust" => Language::Rust,
        "typescript" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "javascript" => Language::JavaScript,
        "jsx" => Language::Jsx,
        "python" => Language::Python,
        _ => Language::Rust,
    }
}

fn parse_kind(raw: &str) -> SymbolKind {
    match raw.trim().to_ascii_lowercase().as_str() {
        "function" => SymbolKind::Function,
        "method" => SymbolKind::Method,
        "class" => SymbolKind::Class,
        "variable" => SymbolKind::Variable,
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "interface" => SymbolKind::Interface,
        "type_alias" => SymbolKind::TypeAlias,
        _ => SymbolKind::Function,
    }
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use aether_config::{
        AetherConfig, EmbeddingProviderKind, EmbeddingVectorBackend, GraphBackend,
        save_workspace_config,
    };
    use aether_store::{SqliteStore, Store, SymbolEmbeddingRecord, SymbolRecord};
    use tempfile::tempdir;

    use super::run_fsck;

    #[test]
    fn fsck_clean_state_reports_zero_inconsistencies() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let store = SqliteStore::open(workspace).expect("open sqlite store");
        let symbol = symbol_record("sym-1", "src/lib.rs", "demo::one");
        store
            .upsert_symbol(symbol.clone())
            .expect("upsert test symbol");
        store
            .write_sir_blob(symbol.id.as_str(), r#"{"intent":"clean"}"#)
            .expect("write sir blob");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: symbol.id.clone(),
                sir_hash: "hash-1".to_owned(),
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                embedding: vec![0.1, 0.2, 0.3],
                updated_at: 1_700_000_000,
            })
            .expect("upsert embedding");

        let report = run_fsck(workspace, false, false).expect("run fsck");
        assert_eq!(report.symbols_in_sqlite, 1);
        assert_eq!(report.vectors_in_store, 1);
        assert_eq!(report.graph_nodes_in_surreal, 0);
        assert_eq!(report.symbols_missing_vectors, 0);
        assert_eq!(report.symbols_missing_graph_nodes, 0);
        assert_eq!(report.phantom_graph_nodes, 0);
        assert_eq!(report.dangling_edges, 0);
        assert_eq!(report.orphaned_vectors, 0);
        assert_eq!(report.incomplete_write_intents, 0);
        assert!(report.graph_check_error.is_some());
    }

    #[test]
    fn fsck_detects_orphaned_and_phantom_records() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_test_config(workspace);

        let store = SqliteStore::open(workspace).expect("open sqlite store");
        let sym_1 = symbol_record("sym-1", "src/lib.rs", "demo::one");
        let sym_2 = symbol_record("sym-2", "src/lib.rs", "demo::two");
        for symbol in [&sym_1, &sym_2] {
            store.upsert_symbol(symbol.clone()).expect("upsert symbol");
            store
                .write_sir_blob(symbol.id.as_str(), r#"{"intent":"orphaned"}"#)
                .expect("write sir blob");
        }

        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: sym_1.id.clone(),
                sir_hash: "hash-1".to_owned(),
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                embedding: vec![0.1, 0.2, 0.3],
                updated_at: 1_700_000_001,
            })
            .expect("upsert valid embedding");
        store
            .upsert_symbol_embedding(SymbolEmbeddingRecord {
                symbol_id: "sym-orphan".to_owned(),
                sir_hash: "hash-orphan".to_owned(),
                provider: "mock".to_owned(),
                model: "mock".to_owned(),
                embedding: vec![0.4, 0.5, 0.6],
                updated_at: 1_700_000_002,
            })
            .expect("upsert orphan embedding");

        let report = run_fsck(workspace, false, false).expect("run fsck");
        assert_eq!(report.symbols_in_sqlite, 2);
        assert_eq!(report.vectors_in_store, 2);
        assert_eq!(report.graph_nodes_in_surreal, 0);
        assert_eq!(report.symbols_missing_vectors, 1);
        assert_eq!(report.symbols_missing_graph_nodes, 0);
        assert_eq!(report.phantom_graph_nodes, 0);
        assert_eq!(report.dangling_edges, 0);
        assert_eq!(report.orphaned_vectors, 1);
        assert_eq!(report.incomplete_write_intents, 0);
        assert!(report.graph_check_error.is_some());
    }

    fn write_test_config(workspace: &Path) {
        let mut config = AetherConfig::default();
        config.storage.graph_backend = GraphBackend::Sqlite;
        config.embeddings.enabled = true;
        config.embeddings.provider = EmbeddingProviderKind::Mock;
        config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
        save_workspace_config(workspace, &config).expect("write config");
    }

    fn symbol_record(id: &str, file_path: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }
}
