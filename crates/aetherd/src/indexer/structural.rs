//! The structural indexer: symbol, edge and test-intent rows written from parsed
//! symbol change events, with symbol removal under the inject lock.

use super::*;

pub(super) struct StructuralIndexer {
    pub(super) workspace_root: PathBuf,
    pub(super) extractor: SymbolExtractor,
    pub(super) test_intent_analyzer: TestIntentAnalyzer,
    pub(super) graph_runtime: tokio::runtime::Runtime,
    pub(super) graph_store: Box<dyn aether_store::GraphStore>,
    pub(super) surreal_graph_store: Option<SurrealGraphStore>,
}

impl StructuralIndexer {
    pub(super) fn new(workspace_root: PathBuf) -> Result<Self> {
        let extractor = SymbolExtractor::new().context("failed to initialize parser")?;
        let test_intent_analyzer = TestIntentAnalyzer::new(&workspace_root)
            .context("failed to initialize test intent analyzer")?;
        let graph_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .context("failed to build graph sync runtime")?;
        let graph_store = graph_runtime
            .block_on(open_graph_store(&workspace_root))
            .context("failed to open configured graph store")?;
        let surreal_graph_store = open_surreal_graph_store_sync(&workspace_root).ok();

        Ok(Self {
            workspace_root,
            extractor,
            test_intent_analyzer,
            graph_runtime,
            graph_store,
            surreal_graph_store,
        })
    }

    pub(super) fn process_event(
        &mut self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
    ) -> Result<()> {
        for symbol in &event.removed {
            // Removal deletes the symbol row and its SIR, so it takes the inject lock the
            // leaf writers hold (as the SIR pipeline's removal does): an `aether_sir_inject`
            // call re-checks the symbol under that lock and cannot persist a leaf for a
            // symbol removed underneath it, which the `sir` table has no foreign key to
            // reject.
            let _inject_guard = acquire_inject_write_lock(&self.workspace_root)?;
            store
                .mark_removed(&symbol.id)
                .with_context(|| format!("failed to mark symbol removed: {}", symbol.id))?;
        }

        let now_ts = unix_timestamp_secs();
        for symbol in event.added.iter().chain(event.updated.iter()) {
            store
                .upsert_symbol(to_symbol_record(symbol, now_ts))
                .with_context(|| format!("failed to upsert symbol {}", symbol.id))?;
        }

        store
            .delete_edges_for_file(&event.file_path)
            .with_context(|| format!("failed to delete edges for file {}", event.file_path))?;

        let full_path = self.workspace_root.join(&event.file_path);
        let source = fs::read_to_string(&full_path);
        match source {
            Ok(source) => {
                let extracted = self
                    .extractor
                    .extract_with_edges_from_path(Path::new(&event.file_path), &source)
                    .with_context(|| format!("failed to extract edges from {}", event.file_path))?;
                store.upsert_edges(&extracted.edges).with_context(|| {
                    format!("failed to upsert edges for file {}", event.file_path)
                })?;
                if let Err(err) = store.populate_symbol_neighbors(event.file_path.as_str()) {
                    tracing::warn!(
                        file_path = %event.file_path,
                        error = %err,
                        "failed to populate symbol_neighbors for file"
                    );
                }
                let now_ms = unix_timestamp_millis();
                let test_intents = extracted
                    .test_intents
                    .into_iter()
                    .map(|intent| to_test_intent_record(intent, now_ms))
                    .collect::<Vec<_>>();
                store
                    .replace_test_intents_for_file(event.file_path.as_str(), &test_intents)
                    .with_context(|| {
                        format!("failed to upsert test intents for file {}", event.file_path)
                    })?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                store
                    .replace_test_intents_for_file(event.file_path.as_str(), &[])
                    .with_context(|| {
                        format!("failed to clear test intents for file {}", event.file_path)
                    })?;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to read source for edge extraction {}",
                        full_path.display()
                    )
                });
            }
        }

        if let Some(graph) = self.surreal_graph_store.as_ref() {
            let _ = self
                .test_intent_analyzer
                .refresh_for_test_file_with_graph(graph, event.file_path.as_str())
                .with_context(|| {
                    format!(
                        "failed to refresh tested_by links for test file {}",
                        event.file_path
                    )
                })?;
        }

        let stats = self
            .graph_runtime
            .block_on(store.sync_graph_for_file(self.graph_store.as_ref(), &event.file_path))
            .with_context(|| format!("failed to sync graph edges for {}", event.file_path))?;
        if stats.unresolved_edges > 0 {
            tracing::debug!(
                file_path = %event.file_path,
                resolved_edges = stats.resolved_edges,
                unresolved_edges = stats.unresolved_edges,
                "graph sync skipped unresolved structural edges"
            );
        }

        Ok(())
    }

    pub(super) fn delete_symbols_batch(&self, symbol_ids: &[String]) -> Result<()> {
        self.graph_runtime
            .block_on(self.graph_store.delete_symbols_batch(symbol_ids))
            .context("failed to delete stale graph symbols")
    }
}

pub(super) fn to_symbol_record(symbol: &Symbol, now_ts: i64) -> SymbolRecord {
    SymbolRecord {
        id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        language: symbol.language.as_str().to_owned(),
        kind: symbol.kind.as_str().to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        signature_fingerprint: symbol.signature_fingerprint.clone(),
        last_seen_at: now_ts,
    }
}

pub(super) fn to_test_intent_record(intent: TestIntent, now_ms: i64) -> TestIntentRecord {
    let material = format!(
        "{}\n{}\n{}",
        intent.file_path.trim(),
        intent.test_name.trim(),
        intent.intent_text.trim(),
    );
    TestIntentRecord {
        intent_id: content_hash(material.as_str()),
        file_path: normalize_path(intent.file_path.as_str()),
        test_name: intent.test_name,
        intent_text: intent.intent_text,
        group_label: intent.group_label,
        language: intent.language.as_str().to_owned(),
        symbol_id: intent.symbol_id,
        created_at: now_ms.max(0),
        updated_at: now_ms.max(0),
    }
}
