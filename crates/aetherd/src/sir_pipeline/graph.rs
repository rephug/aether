//! Graph stage of a processed event: edge replacement and graph synchronization.

use super::*;

impl SirPipeline {
    pub(super) fn finalize_graph_stage(
        &self,
        store: &SqliteStore,
        file_path: &str,
        intents_ready_for_graph: &mut Vec<String>,
    ) {
        match self.sync_graph_for_file(store, file_path) {
            Ok(()) => {
                for intent_id in intents_ready_for_graph.drain(..) {
                    if let Err(err) =
                        store.update_intent_status(&intent_id, WriteIntentStatus::GraphDone)
                    {
                        let message = format!("graph_done update failed: {err:#}");
                        tracing::error!(
                            intent_id = %intent_id,
                            error = %err,
                            "failed to update write intent status to graph_done"
                        );
                        self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                        continue;
                    }
                    if let Err(err) = store.mark_intent_complete(&intent_id) {
                        let message = format!("intent completion failed: {err:#}");
                        tracing::error!(
                            intent_id = %intent_id,
                            error = %err,
                            "failed to mark write intent complete"
                        );
                        self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                    }
                }
            }
            Err(err) => {
                let error_text = format!("{err:#}");
                for intent_id in intents_ready_for_graph.drain(..) {
                    self.mark_intent_failed_safely(store, intent_id.as_str(), error_text.as_str());
                }
                tracing::warn!(
                    file_path = %file_path,
                    error = %error_text,
                    "graph sync failed after vector stage"
                );
            }
        }
    }

    pub(super) fn complete_graph_stage_without_sync(
        &self,
        store: &SqliteStore,
        intents_ready_for_graph: &mut Vec<String>,
    ) {
        for intent_id in intents_ready_for_graph.drain(..) {
            if let Err(err) = store.update_intent_status(&intent_id, WriteIntentStatus::GraphDone) {
                let message = format!("graph_done update failed: {err:#}");
                tracing::error!(
                    intent_id = %intent_id,
                    error = %err,
                    "failed to update write intent status to graph_done"
                );
                self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                continue;
            }
            if let Err(err) = store.mark_intent_complete(&intent_id) {
                let message = format!("intent completion failed: {err:#}");
                tracing::error!(
                    intent_id = %intent_id,
                    error = %err,
                    "failed to mark write intent complete"
                );
                self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
            }
        }
    }

    pub(super) fn batch_complete_graph_stage_without_sync(
        &self,
        store: &SqliteStore,
        intents_by_file: BTreeMap<String, Vec<String>>,
    ) -> BatchCompleteResult {
        let intent_ids = intents_by_file.into_values().flatten().collect::<Vec<_>>();

        if intent_ids.is_empty() {
            return BatchCompleteResult::default();
        }

        match store.batch_complete_intents(&intent_ids) {
            Ok(result) => {
                tracing::info!(
                    intent_count = intent_ids.len(),
                    completed = result.completed,
                    failed = result.failed,
                    "completed batched graph stage without sync"
                );
                result
            }
            Err(err) => {
                let message = format!("batched graph completion failed: {err:#}");
                for intent_id in &intent_ids {
                    self.mark_intent_failed_safely(store, intent_id.as_str(), message.as_str());
                }
                tracing::error!(
                    intent_count = intent_ids.len(),
                    error = %err,
                    "failed to complete batched graph stage without sync"
                );
                BatchCompleteResult {
                    completed: 0,
                    failed: intent_ids.len(),
                }
            }
        }
    }

    pub(super) fn replace_edges_for_file(
        &self,
        store: &SqliteStore,
        event: &SymbolChangeEvent,
    ) -> Result<()> {
        store
            .delete_edges_for_file(&event.file_path)
            .with_context(|| format!("failed to delete edges for file {}", event.file_path))?;

        let full_path = self.workspace_root.join(&event.file_path);
        let source = match fs::read_to_string(&full_path) {
            Ok(source) => Some(source),
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "failed to read source for edge extraction {}",
                        full_path.display()
                    )
                });
            }
        };

        if let Some(source) = source {
            let mut extractor = SymbolExtractor::new().context("failed to initialize parser")?;
            let extracted = extractor
                .extract_with_edges_from_path(Path::new(&event.file_path), &source)
                .with_context(|| format!("failed to extract edges from {}", event.file_path))?;

            store
                .upsert_edges(&extracted.edges)
                .with_context(|| format!("failed to upsert edges for file {}", event.file_path))?;
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
                .replace_test_intents_for_file(event.file_path.as_str(), test_intents.as_slice())
                .with_context(|| {
                    format!("failed to upsert test intents for file {}", event.file_path)
                })?;
        } else {
            store
                .replace_test_intents_for_file(event.file_path.as_str(), &[])
                .with_context(|| {
                    format!("failed to clear test intents for file {}", event.file_path)
                })?;
        }

        if let Ok(graph) = open_surreal_graph_store_sync(&self.workspace_root) {
            let test_intent_analyzer = TestIntentAnalyzer::new(&self.workspace_root)
                .context("failed to initialize test intent analyzer")?;
            let _ = test_intent_analyzer
                .refresh_for_test_file_with_graph(&graph, event.file_path.as_str())
                .with_context(|| {
                    format!(
                        "failed to refresh tested_by links for test file {}",
                        event.file_path
                    )
                })?;
        }

        Ok(())
    }

    pub(super) fn sync_graph_for_file(&self, store: &SqliteStore, file_path: &str) -> Result<()> {
        let stats = self.runtime.block_on(async {
            let graph_store = open_graph_store(&self.workspace_root)
                .await
                .context("failed to open configured graph store")?;
            store
                .sync_graph_for_file(graph_store.as_ref(), file_path)
                .await
                .with_context(|| format!("failed to sync graph edges for file {file_path}"))
        })?;

        if stats.unresolved_edges > 0 {
            tracing::debug!(
                file_path = %file_path,
                resolved_edges = stats.resolved_edges,
                unresolved_edges = stats.unresolved_edges,
                "graph sync skipped unresolved structural edges"
            );
        }

        Ok(())
    }
}
