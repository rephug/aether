#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use aether_core::{EdgeKind, SymbolEdge};
    use async_trait::async_trait;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[derive(Clone)]
    struct CountingEmbeddingProvider {
        calls: Arc<AtomicUsize>,
        purposes: Arc<Mutex<Vec<EmbeddingPurpose>>>,
    }

    #[async_trait]
    impl EmbeddingProvider for CountingEmbeddingProvider {
        async fn embed_text(&self, _text: &str) -> std::result::Result<Vec<f32>, InferError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.purposes
                .lock()
                .expect("purposes mutex")
                .push(EmbeddingPurpose::Document);
            Ok(vec![1.0, 0.0])
        }

        async fn embed_text_with_purpose(
            &self,
            _text: &str,
            purpose: EmbeddingPurpose,
        ) -> std::result::Result<Vec<f32>, InferError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.purposes.lock().expect("purposes mutex").push(purpose);
            Ok(vec![1.0, 0.0])
        }
    }

    struct PanicInferenceProvider;

    #[async_trait]
    impl InferenceProvider for PanicInferenceProvider {
        fn provider_name(&self) -> String {
            "panic".to_owned()
        }

        fn model_name(&self) -> String {
            "panic".to_owned()
        }

        async fn generate_sir(
            &self,
            _symbol_text: &str,
            _context: &SirContext,
        ) -> std::result::Result<SirAnnotation, InferError> {
            panic!("embeddings-only pass must not call inference providers");
        }
    }

    fn write_embeddings_only_config(workspace: &Path) {
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
        )
        .expect("write config");
    }

    fn demo_symbol(symbol_id: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: symbol_id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{symbol_id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    fn demo_sir() -> SirAnnotation {
        SirAnnotation {
            intent: "Demo intent".to_owned(),
            inputs: vec!["input".to_owned()],
            outputs: vec!["output".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.9,
        }
    }

    fn seed_sir(store: &SqliteStore, symbol_id: &str, sir: &SirAnnotation) -> String {
        let canonical = canonicalize_sir_json(sir);
        let hash = sir_hash(sir);
        store
            .write_sir_blob(symbol_id, &canonical)
            .expect("write sir blob");
        store
            .upsert_sir_meta(SirMetaRecord {
                id: symbol_id.to_owned(),
                sir_hash: hash.clone(),
                sir_version: 1,
                provider: "test".to_owned(),
                model: "test".to_owned(),
                generation_pass: SIR_GENERATION_PASS_SCAN.to_owned(),
                updated_at: 1_700_000_100,
                sir_status: SIR_STATUS_FRESH.to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_100,
            })
            .expect("upsert sir meta");
        hash
    }

    fn build_embeddings_only_pipeline(
        workspace: &Path,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> SirPipeline {
        SirPipeline::new_with_provider_and_embeddings(
            workspace.to_path_buf(),
            1,
            Arc::new(PanicInferenceProvider),
            "panic",
            "panic",
            Some(embedding_provider),
            Some(("test_embedding".to_owned(), "test-model".to_owned())),
            None,
            None,
        )
        .expect("build pipeline")
    }

    fn upsert_existing_embedding(workspace: &Path, symbol_id: &str, sir_hash: &str) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let vector_store = runtime
            .block_on(open_vector_store(workspace))
            .expect("open vector store");
        runtime
            .block_on(vector_store.upsert_embedding(SymbolEmbeddingRecord {
                symbol_id: symbol_id.to_owned(),
                sir_hash: sir_hash.to_owned(),
                provider: "test_embedding".to_owned(),
                model: "test-model".to_owned(),
                embedding: vec![1.0, 0.0],
                updated_at: 1_700_000_200,
            }))
            .expect("seed embedding");
    }

    fn count_table_rows(workspace: &Path, table: &str) -> i64 {
        let conn =
            Connection::open(workspace.join(".aether/meta.sqlite")).expect("open sqlite database");
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
    }

    #[test]
    fn new_embeddings_only_errors_when_provider_is_not_configured() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
        fs::write(
            workspace.join(".aether/config.toml"),
            r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
"#,
        )
        .expect("write config");

        let err = match SirPipeline::new_embeddings_only(workspace.to_path_buf()) {
            Ok(_) => panic!("missing embedding config should error"),
            Err(err) => err,
        };

        assert_eq!(
            err.to_string(),
            "Embedding provider is not configured. Set [embeddings] in config."
        );
    }

    #[test]
    fn embeddings_only_calls_embedding_provider_not_inference() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let sir = demo_sir();
        for (symbol_id, qualified_name) in [
            ("sym-a", "demo::a"),
            ("sym-b", "demo::b"),
            ("sym-c", "demo::c"),
        ] {
            store
                .upsert_symbol(demo_symbol(symbol_id, qualified_name))
                .expect("upsert symbol");
            seed_sir(&store, symbol_id, &sir);
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[
                EmbeddingPurpose::Document,
                EmbeddingPurpose::Document,
                EmbeddingPurpose::Document,
            ]
        );
        let rendered = String::from_utf8(out).expect("utf8 output");
        assert!(rendered.contains("Re-embedding 3 symbols with test_embedding/test-model..."));
        assert!(rendered.contains(
            "Re-embedded 3 of 3 symbols with test_embedding/test-model (0 skipped: no current SIR, 0 already up to date, 0 errors)"
        ));
    }

    #[test]
    fn embeddings_only_respects_skip_logic() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let sir = demo_sir();
        let mut hashes = HashMap::new();
        for (symbol_id, qualified_name) in [
            ("sym-a", "demo::a"),
            ("sym-b", "demo::b"),
            ("sym-c", "demo::c"),
        ] {
            store
                .upsert_symbol(demo_symbol(symbol_id, qualified_name))
                .expect("upsert symbol");
            hashes.insert(symbol_id.to_owned(), seed_sir(&store, symbol_id, &sir));
        }

        upsert_existing_embedding(workspace, "sym-a", hashes["sym-a"].as_str());
        upsert_existing_embedding(workspace, "sym-b", hashes["sym-b"].as_str());

        let calls = Arc::new(AtomicUsize::new(0));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[EmbeddingPurpose::Document]
        );
        let rendered = String::from_utf8(out).expect("utf8 output");
        assert!(rendered.contains(
            "Re-embedded 1 of 3 symbols with test_embedding/test-model (0 skipped: no current SIR, 2 already up to date, 0 errors)"
        ));
    }

    #[test]
    fn embeddings_only_skips_symbols_without_sir() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        store
            .upsert_symbol(demo_symbol("sym-with-sir", "demo::with_sir"))
            .expect("upsert symbol with sir");
        store
            .upsert_symbol(demo_symbol("sym-without-sir", "demo::without_sir"))
            .expect("upsert symbol without sir");

        let sir = demo_sir();
        seed_sir(&store, "sym-with-sir", &sir);

        let calls = Arc::new(AtomicUsize::new(0));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        let stored = store
            .get_symbol_embedding_meta("sym-with-sir")
            .expect("read stored embedding meta");
        assert!(stored.is_some());
        let missing = store
            .get_symbol_embedding_meta("sym-without-sir")
            .expect("read missing embedding meta");
        assert!(missing.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[EmbeddingPurpose::Document]
        );

        let rendered = String::from_utf8(out).expect("utf8 output");
        assert!(rendered.contains(
            "Re-embedded 1 of 2 symbols with test_embedding/test-model (1 skipped: no current SIR, 0 already up to date, 0 errors)"
        ));
    }

    #[test]
    fn embeddings_only_does_not_mutate_non_embedding_state() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let sir = demo_sir();
        for (symbol_id, qualified_name) in [("sym-a", "demo::a"), ("sym-b", "demo::b")] {
            store
                .upsert_symbol(demo_symbol(symbol_id, qualified_name))
                .expect("upsert symbol");
            seed_sir(&store, symbol_id, &sir);
        }
        store
            .upsert_edges(&[SymbolEdge {
                source_id: "sym-a".to_owned(),
                target_qualified_name: "demo::b".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            }])
            .expect("upsert edges");

        let before_symbols = count_table_rows(workspace, "symbols");
        let before_sir = count_table_rows(workspace, "sir");
        let before_edges = count_table_rows(workspace, "symbol_edges");

        let calls = Arc::new(AtomicUsize::new(0));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[EmbeddingPurpose::Document, EmbeddingPurpose::Document]
        );
        assert_eq!(count_table_rows(workspace, "symbols"), before_symbols);
        assert_eq!(count_table_rows(workspace, "sir"), before_sir);
        assert_eq!(count_table_rows(workspace, "symbol_edges"), before_edges);
    }
}
