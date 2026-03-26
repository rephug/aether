#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use aether_core::{
        EdgeKind, Language, Position, SourceRange, Symbol, SymbolEdge, SymbolKind, content_hash,
    };
    use aether_store::SemanticIndexStore;
    use async_trait::async_trait;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[derive(Clone)]
    struct CountingEmbeddingProvider {
        calls: Arc<AtomicUsize>,
        batch_calls: Arc<AtomicUsize>,
        batch_sizes: Arc<Mutex<Vec<usize>>>,
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

        async fn embed_texts_with_purpose(
            &self,
            texts: &[&str],
            purpose: EmbeddingPurpose,
        ) -> std::result::Result<Vec<Vec<f32>>, InferError> {
            self.batch_calls.fetch_add(1, Ordering::SeqCst);
            self.batch_sizes
                .lock()
                .expect("batch sizes mutex")
                .push(texts.len());
            self.purposes.lock().expect("purposes mutex").push(purpose);
            Ok(vec![vec![1.0, 0.0]; texts.len()])
        }
    }

    struct PanicInferenceProvider;

    #[derive(Clone)]
    struct FixedInferenceProvider {
        sir: SirAnnotation,
    }

    #[derive(Clone)]
    struct CountingInferenceProvider {
        symbol_sir: SirAnnotation,
        prompt_sir: SirAnnotation,
        standard_calls: Arc<AtomicUsize>,
        prompt_calls: Arc<AtomicUsize>,
        file_calls: Arc<AtomicUsize>,
    }

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

    #[async_trait]
    impl InferenceProvider for FixedInferenceProvider {
        fn provider_name(&self) -> String {
            "fixed".to_owned()
        }

        fn model_name(&self) -> String {
            "fixed-model".to_owned()
        }

        async fn generate_sir(
            &self,
            _symbol_text: &str,
            _context: &SirContext,
        ) -> std::result::Result<SirAnnotation, InferError> {
            Ok(self.sir.clone())
        }

        async fn generate_sir_from_prompt(
            &self,
            _prompt: &str,
            _context: &SirContext,
            _deep_mode: bool,
        ) -> std::result::Result<SirAnnotation, InferError> {
            Ok(self.sir.clone())
        }
    }

    #[async_trait]
    impl InferenceProvider for CountingInferenceProvider {
        fn provider_name(&self) -> String {
            "counting".to_owned()
        }

        fn model_name(&self) -> String {
            "counting-model".to_owned()
        }

        async fn generate_sir(
            &self,
            _symbol_text: &str,
            context: &SirContext,
        ) -> std::result::Result<SirAnnotation, InferError> {
            if context.kind == "file" {
                self.file_calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.prompt_sir.clone())
            } else {
                self.standard_calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.symbol_sir.clone())
            }
        }

        async fn generate_sir_from_prompt(
            &self,
            _prompt: &str,
            context: &SirContext,
            _deep_mode: bool,
        ) -> std::result::Result<SirAnnotation, InferError> {
            if context.kind == "file" {
                self.file_calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.prompt_sir.clone())
            } else {
                self.prompt_calls.fetch_add(1, Ordering::SeqCst);
                Ok(self.symbol_sir.clone())
            }
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

    fn demo_symbol_record_with_kind(
        symbol_id: &str,
        qualified_name: &str,
        kind: &str,
        file_path: &str,
    ) -> SymbolRecord {
        SymbolRecord {
            id: symbol_id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: kind.to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{symbol_id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    fn demo_type_symbol(
        symbol_id: &str,
        name: &str,
        qualified_name: &str,
        file_path: &str,
        kind: SymbolKind,
        source: &str,
    ) -> Symbol {
        Symbol {
            id: symbol_id.to_owned(),
            language: Language::Rust,
            file_path: file_path.to_owned(),
            kind,
            name: name.to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{symbol_id}"),
            content_hash: content_hash(source),
            range: SourceRange {
                start: Position { line: 1, column: 1 },
                end: Position {
                    line: source.lines().count().max(1),
                    column: source.lines().last().map(|line| line.len() + 1).unwrap_or(1),
                },
                start_byte: Some(0),
                end_byte: Some(source.len()),
            },
        }
    }

    fn demo_sir() -> SirAnnotation {
        SirAnnotation {
            intent: "Demo intent".to_owned(),
            behavior: None,
            inputs: vec!["input".to_owned()],
            outputs: vec!["output".to_owned()],
            side_effects: Vec::new(),
            dependencies: Vec::new(),
            error_modes: Vec::new(),
            confidence: 0.9,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        }
    }

    fn demo_rollup_sir() -> SirAnnotation {
        SirAnnotation {
            intent: "File rollup summary".to_owned(),
            ..demo_sir()
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
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
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

    fn build_write_pipeline(workspace: &Path, provider: Arc<dyn InferenceProvider>) -> SirPipeline {
        build_write_pipeline_with_embeddings(workspace, provider, None)
    }

    fn build_write_pipeline_with_embeddings(
        workspace: &Path,
        provider: Arc<dyn InferenceProvider>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    ) -> SirPipeline {
        let embedding_identity = embedding_provider
            .as_ref()
            .map(|_| ("test_embedding".to_owned(), "test-model".to_owned()));
        SirPipeline::new_with_provider_and_embeddings(
            workspace.to_path_buf(),
            1,
            provider,
            "test_provider",
            "test_model",
            embedding_provider,
            embedding_identity,
            None,
            None,
        )
        .expect("build pipeline")
    }

    fn make_quality_batch_items(symbols: &[Symbol]) -> Vec<QualityBatchItem> {
        symbols
            .iter()
            .cloned()
            .map(|symbol| QualityBatchItem {
                symbol,
                priority_score: 0.9,
                enrichment: SirEnrichmentContext {
                    file_intent: None,
                    neighbor_intents: Vec::new(),
                    baseline_sir: None,
                    priority_reason: "test batch".to_owned(),
                    caller_contract_clauses: Vec::new(),
                },
                use_cot: false,
            })
            .collect()
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

    fn install_graph_done_failure_trigger(workspace: &Path, symbol_id: &str) {
        let conn =
            Connection::open(workspace.join(".aether/meta.sqlite")).expect("open sqlite database");
        conn.execute_batch(
            format!(
                r#"
                CREATE TRIGGER fail_graph_done_for_test
                BEFORE UPDATE OF status ON write_intents
                WHEN NEW.status = 'graph_done' AND OLD.symbol_id = '{symbol_id}'
                BEGIN
                    SELECT RAISE(FAIL, 'graph_done blocked for test');
                END;
                "#
            )
            .as_str(),
        )
        .expect("install graph_done failure trigger");
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
    fn commit_successful_generation_injects_method_dependencies_from_symbol_edges_across_files() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        for symbol in [
            demo_symbol_record_with_kind("sym-store", "Store", "trait", "src/store.rs"),
            demo_symbol_record_with_kind("sym-load", "Store::load", "method", "src/load.rs"),
            demo_symbol_record_with_kind("sym-save", "Store::save", "method", "src/save.rs"),
        ] {
            store.upsert_symbol(symbol).expect("upsert symbol");
        }
        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: "sym-load".to_owned(),
                    target_qualified_name: "Helper".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/load.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: "sym-load".to_owned(),
                    target_qualified_name: "Record".to_owned(),
                    edge_kind: EdgeKind::TypeRef,
                    file_path: "src/load.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: "sym-load".to_owned(),
                    target_qualified_name: "StoreError".to_owned(),
                    edge_kind: EdgeKind::TypeRef,
                    file_path: "src/load.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: "sym-save".to_owned(),
                    target_qualified_name: "Record".to_owned(),
                    edge_kind: EdgeKind::TypeRef,
                    file_path: "src/save.rs".to_owned(),
                },
            ])
            .expect("upsert edges");

        let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
        let parent_symbol = demo_type_symbol(
            "sym-store",
            "Store",
            "Store",
            "src/store.rs",
            SymbolKind::Trait,
            "pub trait Store {\n    fn load(&self) -> Record;\n    fn save(&self, record: Record);\n}\n",
        );
        let generated = infer::GeneratedSir {
            symbol: parent_symbol.clone(),
            sir: SirAnnotation {
                intent: "Storage interface".to_owned(),
                behavior: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: vec!["stale".to_owned()],
                error_modes: Vec::new(),
                confidence: 0.9,
                edge_cases: None,
                complexity: None,
                method_dependencies: Some(HashMap::from([(
                    "stale".to_owned(),
                    vec!["stale".to_owned()],
                )])),
            },
            provider_name: "test_provider".to_owned(),
            model_name: "test_model".to_owned(),
            reasoning_trace: None,
        };

        let mut out = Vec::new();
        let intent_id = pipeline
            .commit_successful_generation(
                &store,
                generated,
                SIR_GENERATION_PASS_SCAN,
                None,
                false,
                &mut out,
            )
            .expect("commit successful generation")
            .expect("intent id");

        let stored_blob = store
            .read_sir_blob(parent_symbol.id.as_str())
            .expect("read sir blob")
            .expect("sir blob should exist");
        let stored_sir: SirAnnotation =
            serde_json::from_str(&stored_blob).expect("stored sir should deserialize");
        let method_dependencies = stored_sir
            .method_dependencies
            .expect("method dependencies should be injected");
        assert_eq!(
            method_dependencies.get("load"),
            Some(&vec![
                "Helper".to_owned(),
                "Record".to_owned(),
                "StoreError".to_owned(),
            ])
        );
        assert_eq!(
            method_dependencies.get("save"),
            Some(&vec!["Record".to_owned()])
        );
        assert_eq!(
            stored_sir.dependencies,
            vec![
                "Helper".to_owned(),
                "Record".to_owned(),
                "StoreError".to_owned(),
                "stale".to_owned(),
            ]
        );

        let intent = store
            .get_intent(intent_id.as_str())
            .expect("read intent")
            .expect("intent should exist");
        assert_eq!(intent.status, WriteIntentStatus::VectorDone);
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
    fn persist_sir_payload_updates_metadata_when_hash_is_unchanged() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);

        let store = SqliteStore::open(workspace).expect("open store");
        let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
        let symbol = demo_type_symbol(
            "sym-triage",
            "run",
            "demo::run",
            "src/lib.rs",
            SymbolKind::Function,
            "fn run() {}\n",
        );
        let sir = demo_sir();

        pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol.clone(),
                    sir: sir.clone(),
                    provider_name: "scan-provider".to_owned(),
                    model_name: "scan-model".to_owned(),
                    generation_pass: SIR_GENERATION_PASS_SCAN.to_owned(),
                    reasoning_trace: None,
                    commit_hash: None,
                },
            )
            .expect("persist scan payload");

        let (canonical_json, sir_hash_value) = pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol.clone(),
                    sir: sir.clone(),
                    provider_name: "triage-provider".to_owned(),
                    model_name: "triage-model".to_owned(),
                    generation_pass: SIR_GENERATION_PASS_TRIAGE.to_owned(),
                    reasoning_trace: Some("triage reasoning".to_owned()),
                    commit_hash: None,
                },
            )
            .expect("persist triage payload");

        assert_eq!(canonical_json, canonicalize_sir_json(&sir));
        assert_eq!(sir_hash_value, sir_hash(&sir));

        let meta = store
            .get_sir_meta(symbol.id.as_str())
            .expect("load sir meta")
            .expect("sir meta exists");
        assert_eq!(meta.sir_hash, sir_hash_value);
        assert_eq!(meta.sir_version, 1);
        assert_eq!(meta.provider, "triage-provider");
        assert_eq!(meta.model, "triage-model");
        assert_eq!(meta.generation_pass, SIR_GENERATION_PASS_TRIAGE);
        assert_eq!(meta.reasoning_trace.as_deref(), Some("triage reasoning"));

        let history = store
            .list_sir_history(symbol.id.as_str())
            .expect("load sir history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].sir_hash, sir_hash_value);
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
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                batch_calls: Arc::clone(&batch_calls),
                batch_sizes: Arc::clone(&batch_sizes),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(batch_calls.load(Ordering::SeqCst), 0);
        assert!(batch_sizes.lock().expect("batch sizes mutex").is_empty());
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
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                batch_calls: Arc::clone(&batch_calls),
                batch_sizes: Arc::clone(&batch_sizes),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(batch_calls.load(Ordering::SeqCst), 0);
        assert!(batch_sizes.lock().expect("batch sizes mutex").is_empty());
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
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                batch_calls: Arc::clone(&batch_calls),
                batch_sizes: Arc::clone(&batch_sizes),
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
        assert_eq!(batch_calls.load(Ordering::SeqCst), 0);
        assert!(batch_sizes.lock().expect("batch sizes mutex").is_empty());
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
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_embeddings_only_pipeline(
            workspace,
            Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                batch_calls: Arc::clone(&batch_calls),
                batch_sizes: Arc::clone(&batch_sizes),
                purposes: Arc::clone(&purposes),
            }),
        );

        let mut out = Vec::new();
        pipeline
            .run_embeddings_only_pass(&store, false, &mut out)
            .expect("run embeddings-only pass");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(batch_calls.load(Ordering::SeqCst), 0);
        assert!(batch_sizes.lock().expect("batch sizes mutex").is_empty());
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[EmbeddingPurpose::Document, EmbeddingPurpose::Document]
        );
        assert_eq!(count_table_rows(workspace, "symbols"), before_symbols);
        assert_eq!(count_table_rows(workspace, "sir"), before_sir);
        assert_eq!(count_table_rows(workspace, "symbol_edges"), before_edges);
    }

    #[test]
    fn process_event_with_skip_surreal_sync_refreshes_local_edges_and_completes_intents() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        fs::create_dir_all(workspace.join("src")).expect("create src");
        let source = r#"
pub struct Store;
pub struct Record;

fn helper(record: &Record) {}

impl Store {
    pub fn load(&self) -> Record {
        helper(&Record);
        Record
    }

    pub fn save(&self, record: Record) {
        helper(&record);
    }
}
"#;
        fs::write(workspace.join("src/lib.rs"), source).expect("write source");

        let store = SqliteStore::open(workspace).expect("open store");
        let mut extractor = SymbolExtractor::new().expect("symbol extractor");
        let extracted = extractor
            .extract_with_edges_from_path(Path::new("src/lib.rs"), source)
            .expect("extract source");
        for symbol in &extracted.symbols {
            store
                .upsert_symbol(demo_symbol_record_with_kind(
                    symbol.id.as_str(),
                    symbol.qualified_name.as_str(),
                    symbol.kind.as_str(),
                    symbol.file_path.as_str(),
                ))
                .expect("upsert symbol");
        }
        let parent_symbol = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "Store")
            .expect("store symbol")
            .clone();
        let load_symbol = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "Store::load")
            .expect("load symbol");
        let save_symbol = extracted
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "Store::save")
            .expect("save symbol");
        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: load_symbol.id.clone(),
                    target_qualified_name: "StaleLoader".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: save_symbol.id.clone(),
                    target_qualified_name: "StaleRecord".to_owned(),
                    edge_kind: EdgeKind::TypeRef,
                    file_path: "src/lib.rs".to_owned(),
                },
            ])
            .expect("upsert edges");

        let provider = Arc::new(FixedInferenceProvider {
            sir: SirAnnotation {
                intent: "Storage interface".to_owned(),
                behavior: None,
                inputs: Vec::new(),
                outputs: Vec::new(),
                side_effects: Vec::new(),
                dependencies: Vec::new(),
                error_modes: Vec::new(),
                confidence: 0.9,
                edge_cases: None,
                complexity: None,
                method_dependencies: None,
            },
        });
        let pipeline = build_write_pipeline(workspace, provider).with_skip_surreal_sync(true);
        let event = SymbolChangeEvent {
            file_path: "src/lib.rs".to_owned(),
            language: Language::Rust,
            added: Vec::new(),
            removed: Vec::new(),
            updated: vec![parent_symbol.clone()],
        };

        let mut out = Vec::new();
        let stats = pipeline
            .process_event_with_priority_and_pass(
                &store,
                &event,
                true,
                false,
                &mut out,
                None,
                SIR_GENERATION_PASS_REGENERATED,
            )
            .expect("process event");

        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.failure_count, 0);
        let refreshed_edges = store
            .list_symbol_edges_for_source_and_kinds(
                load_symbol.id.as_str(),
                &[EdgeKind::Calls, EdgeKind::TypeRef],
            )
            .expect("read refreshed edges");
        assert!(
            refreshed_edges
                .iter()
                .any(|edge| edge.target_qualified_name == "helper")
        );
        assert!(
            refreshed_edges
                .iter()
                .any(|edge| edge.target_qualified_name == "Record")
        );
        assert!(
            refreshed_edges
                .iter()
                .all(|edge| edge.target_qualified_name != "StaleLoader")
        );
        assert!(store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty());
        assert_eq!(
            store
                .count_intents_by_status()
                .expect("count intents")
                .get("complete"),
            Some(&1usize)
        );

        let stored_blob = store
            .read_sir_blob(parent_symbol.id.as_str())
            .expect("read sir blob")
            .expect("sir blob should exist");
        let stored_sir: SirAnnotation =
            serde_json::from_str(&stored_blob).expect("stored sir should deserialize");
        let method_dependencies = stored_sir
            .method_dependencies
            .expect("method dependencies should be injected");
        assert_eq!(
            method_dependencies.get("load"),
            Some(&vec!["Record".to_owned(), "helper".to_owned()])
        );
        assert_eq!(
            method_dependencies.get("save"),
            Some(&vec!["Record".to_owned(), "helper".to_owned()])
        );
        assert_eq!(
            stored_sir.dependencies,
            vec!["Record".to_owned(), "helper".to_owned()]
        );
    }

    #[test]
    fn process_bulk_scan_batches_embeddings_and_completes_intents() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        fs::create_dir_all(workspace.join("src")).expect("create src");

        let mut source = String::new();
        for idx in 0..105 {
            source.push_str(format!("pub fn symbol_{idx}() -> i32 {{ {idx} }}\n").as_str());
        }
        fs::write(workspace.join("src/lib.rs"), &source).expect("write source");

        let mut extractor = SymbolExtractor::new().expect("symbol extractor");
        let extracted = extractor
            .extract_with_edges_from_path(Path::new("src/lib.rs"), &source)
            .expect("extract source");
        let symbols = extracted.symbols;

        let store = SqliteStore::open(workspace).expect("open store");
        for symbol in &symbols {
            store
                .upsert_symbol(demo_symbol_record_with_kind(
                    symbol.id.as_str(),
                    symbol.qualified_name.as_str(),
                    symbol.kind.as_str(),
                    symbol.file_path.as_str(),
                ))
                .expect("upsert symbol");
        }

        let priority_scores = symbols
            .iter()
            .enumerate()
            .map(|(idx, symbol)| (symbol.id.clone(), idx as f64))
            .collect::<HashMap<_, _>>();
        let standard_calls = Arc::new(AtomicUsize::new(0));
        let prompt_calls = Arc::new(AtomicUsize::new(0));
        let file_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_write_pipeline_with_embeddings(
            workspace,
            Arc::new(CountingInferenceProvider {
                symbol_sir: demo_sir(),
                prompt_sir: demo_rollup_sir(),
                standard_calls: Arc::clone(&standard_calls),
                prompt_calls: Arc::clone(&prompt_calls),
                file_calls: Arc::clone(&file_calls),
            }),
            Some(Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                batch_calls: Arc::clone(&batch_calls),
                batch_sizes: Arc::clone(&batch_sizes),
                purposes: Arc::clone(&purposes),
            })),
        )
        .with_skip_surreal_sync(true);

        let mut out = Vec::new();
        let stats = pipeline
            .process_bulk_scan(
                &store,
                symbols.clone(),
                &priority_scores,
                false,
                SIR_GENERATION_PASS_SCAN,
                false,
                &mut out,
            )
            .expect("process bulk scan");

        assert_eq!(stats.success_count, symbols.len());
        assert_eq!(stats.failure_count, 0);
        assert_eq!(standard_calls.load(Ordering::SeqCst), symbols.len());
        assert_eq!(prompt_calls.load(Ordering::SeqCst), 0);
        assert_eq!(file_calls.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(batch_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            batch_sizes.lock().expect("batch sizes mutex").as_slice(),
            &[100, 5]
        );
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[EmbeddingPurpose::Document, EmbeddingPurpose::Document]
        );
        assert!(store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty());
        assert_eq!(
            store
                .count_intents_by_status()
                .expect("count intents")
                .get("complete"),
            Some(&symbols.len())
        );
        for symbol in &symbols {
            assert!(store
                .get_symbol_embedding_meta(symbol.id.as_str())
                .expect("read embedding meta")
                .is_some());
        }

        let rollup_id = synthetic_file_sir_id("rust", "src/lib.rs");
        assert!(store
            .read_sir_blob(rollup_id.as_str())
            .expect("read rollup blob")
            .is_some());
    }

    #[test]
    fn process_bulk_scan_uses_local_rollup_for_small_files() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        fs::create_dir_all(workspace.join("src")).expect("create src");

        let mut source = String::new();
        for idx in 0..5 {
            source.push_str(format!("pub fn symbol_{idx}() -> i32 {{ {idx} }}\n").as_str());
        }
        fs::write(workspace.join("src/lib.rs"), &source).expect("write source");

        let mut extractor = SymbolExtractor::new().expect("symbol extractor");
        let extracted = extractor
            .extract_with_edges_from_path(Path::new("src/lib.rs"), &source)
            .expect("extract source");
        let symbols = extracted.symbols;

        let store = SqliteStore::open(workspace).expect("open store");
        for symbol in &symbols {
            store
                .upsert_symbol(demo_symbol_record_with_kind(
                    symbol.id.as_str(),
                    symbol.qualified_name.as_str(),
                    symbol.kind.as_str(),
                    symbol.file_path.as_str(),
                ))
                .expect("upsert symbol");
        }

        let priority_scores = symbols
            .iter()
            .enumerate()
            .map(|(idx, symbol)| (symbol.id.clone(), idx as f64))
            .collect::<HashMap<_, _>>();
        let standard_calls = Arc::new(AtomicUsize::new(0));
        let prompt_calls = Arc::new(AtomicUsize::new(0));
        let file_calls = Arc::new(AtomicUsize::new(0));
        let pipeline = build_write_pipeline(
            workspace,
            Arc::new(CountingInferenceProvider {
                symbol_sir: demo_sir(),
                prompt_sir: demo_rollup_sir(),
                standard_calls: Arc::clone(&standard_calls),
                prompt_calls: Arc::clone(&prompt_calls),
                file_calls: Arc::clone(&file_calls),
            }),
        )
        .with_skip_surreal_sync(true);

        let mut out = Vec::new();
        let stats = pipeline
            .process_bulk_scan(
                &store,
                symbols.clone(),
                &priority_scores,
                false,
                SIR_GENERATION_PASS_SCAN,
                false,
                &mut out,
            )
            .expect("process bulk scan");

        assert_eq!(stats.success_count, symbols.len());
        assert_eq!(stats.failure_count, 0);
        assert_eq!(standard_calls.load(Ordering::SeqCst), symbols.len());
        assert_eq!(prompt_calls.load(Ordering::SeqCst), 0);
        assert_eq!(file_calls.load(Ordering::SeqCst), 0);
        assert!(store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty());

        let rollup_id = synthetic_file_sir_id("rust", "src/lib.rs");
        assert!(store
            .read_sir_blob(rollup_id.as_str())
            .expect("read rollup blob")
            .is_some());
    }

    #[test]
    fn process_bulk_scan_counts_batched_graph_completion_failures() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        fs::create_dir_all(workspace.join("src")).expect("create src");

        let mut source = String::new();
        for idx in 0..2 {
            source.push_str(format!("pub fn symbol_{idx}() -> i32 {{ {idx} }}\n").as_str());
        }
        fs::write(workspace.join("src/lib.rs"), &source).expect("write source");

        let mut extractor = SymbolExtractor::new().expect("symbol extractor");
        let extracted = extractor
            .extract_with_edges_from_path(Path::new("src/lib.rs"), &source)
            .expect("extract source");
        let symbols = extracted.symbols;

        let store = SqliteStore::open(workspace).expect("open store");
        for symbol in &symbols {
            store
                .upsert_symbol(demo_symbol_record_with_kind(
                    symbol.id.as_str(),
                    symbol.qualified_name.as_str(),
                    symbol.kind.as_str(),
                    symbol.file_path.as_str(),
                ))
                .expect("upsert symbol");
        }

        install_graph_done_failure_trigger(workspace, symbols[0].id.as_str());

        let priority_scores = symbols
            .iter()
            .enumerate()
            .map(|(idx, symbol)| (symbol.id.clone(), idx as f64))
            .collect::<HashMap<_, _>>();
        let pipeline = build_write_pipeline(workspace, Arc::new(FixedInferenceProvider { sir: demo_sir() }))
            .with_skip_surreal_sync(true);

        let mut out = Vec::new();
        let stats = pipeline
            .process_bulk_scan(
                &store,
                symbols.clone(),
                &priority_scores,
                false,
                SIR_GENERATION_PASS_SCAN,
                false,
                &mut out,
            )
            .expect("process bulk scan");

        assert_eq!(stats.success_count, symbols.len());
        assert_eq!(stats.failure_count, 1);
        assert_eq!(
            store
                .count_intents_by_status()
                .expect("count intents")
                .get("complete"),
            Some(&1usize)
        );
        assert_eq!(
            store
                .count_intents_by_status()
                .expect("count intents")
                .get("failed"),
            Some(&1usize)
        );
    }

    #[test]
    fn process_quality_batch_batches_embeddings_and_completes_intents() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        fs::create_dir_all(workspace.join("src")).expect("create src");

        let mut source = String::new();
        for idx in 0..105 {
            source.push_str(format!("pub fn symbol_{idx}() -> i32 {{ {idx} }}\n").as_str());
        }
        fs::write(workspace.join("src/lib.rs"), &source).expect("write source");

        let mut extractor = SymbolExtractor::new().expect("symbol extractor");
        let extracted = extractor
            .extract_with_edges_from_path(Path::new("src/lib.rs"), &source)
            .expect("extract source");
        let symbols = extracted.symbols;

        let store = SqliteStore::open(workspace).expect("open store");
        for symbol in &symbols {
            store
                .upsert_symbol(demo_symbol_record_with_kind(
                    symbol.id.as_str(),
                    symbol.qualified_name.as_str(),
                    symbol.kind.as_str(),
                    symbol.file_path.as_str(),
                ))
                .expect("upsert symbol");
        }

        let standard_calls = Arc::new(AtomicUsize::new(0));
        let prompt_calls = Arc::new(AtomicUsize::new(0));
        let file_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let batch_calls = Arc::new(AtomicUsize::new(0));
        let batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let purposes = Arc::new(Mutex::new(Vec::new()));
        let pipeline = build_write_pipeline_with_embeddings(
            workspace,
            Arc::new(CountingInferenceProvider {
                symbol_sir: demo_sir(),
                prompt_sir: demo_rollup_sir(),
                standard_calls: Arc::clone(&standard_calls),
                prompt_calls: Arc::clone(&prompt_calls),
                file_calls: Arc::clone(&file_calls),
            }),
            Some(Arc::new(CountingEmbeddingProvider {
                calls: Arc::clone(&calls),
                batch_calls: Arc::clone(&batch_calls),
                batch_sizes: Arc::clone(&batch_sizes),
                purposes: Arc::clone(&purposes),
            })),
        )
        .with_skip_surreal_sync(true);

        let mut out = Vec::new();
        let stats = pipeline
            .process_quality_batch(
                &store,
                make_quality_batch_items(&symbols),
                SIR_GENERATION_PASS_TRIAGE,
                false,
                &mut out,
            )
            .expect("process quality batch");

        assert_eq!(stats.success_count, symbols.len());
        assert_eq!(stats.failure_count, 0);
        assert_eq!(standard_calls.load(Ordering::SeqCst), 0);
        assert_eq!(prompt_calls.load(Ordering::SeqCst), symbols.len());
        assert_eq!(file_calls.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(batch_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            batch_sizes.lock().expect("batch sizes mutex").as_slice(),
            &[100, 5]
        );
        assert_eq!(
            purposes.lock().expect("purposes mutex").as_slice(),
            &[EmbeddingPurpose::Document, EmbeddingPurpose::Document]
        );
        assert!(store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty());
        assert_eq!(
            store
                .count_intents_by_status()
                .expect("count intents")
                .get("complete"),
            Some(&symbols.len())
        );
        for symbol in &symbols {
            assert!(store
                .get_symbol_embedding_meta(symbol.id.as_str())
                .expect("read embedding meta")
                .is_some());
        }

        let rollup_id = synthetic_file_sir_id("rust", "src/lib.rs");
        assert!(store
            .read_sir_blob(rollup_id.as_str())
            .expect("read rollup blob")
            .is_some());
    }
}
