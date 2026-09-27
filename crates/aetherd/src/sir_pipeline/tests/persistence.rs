//! Persisting generated SIRs: the commit path, metadata updates, identity guards, rollback,
//! a write superseded during its embedding, and write generations.

use super::*;

#[test]
fn commit_successful_generation_injects_method_dependencies_from_symbol_edges_across_files() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let parent_source = "pub trait Store {\n    fn load(&self) -> Record;\n    fn save(&self, record: Record);\n}\n";
    let parent_symbol = parsed_symbols(workspace, "src/store.rs", parent_source)
        .into_iter()
        .find(|symbol| symbol.kind == SymbolKind::Trait)
        .expect("the source declares the trait");
    for symbol in [
        demo_symbol_record_with_kind(&parent_symbol.id, "Store", "trait", "src/store.rs"),
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
        prior_sir: None,
        source_hash: parent_symbol.content_hash.clone(),
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
                prompt_hash: None,
                source_hash: None,
                prior_sir: PriorSir::Unrecorded,
            },
            None,
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
                prompt_hash: None,
                source_hash: None,
                prior_sir: PriorSir::Unrecorded,
            },
            None,
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
fn persist_successful_generation_sqlite_skips_a_sir_whose_content_cycled_back() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn cycle() {}\n");
    store.upsert_symbol(record).expect("upsert symbol");
    let symbol_id = symbol.id.as_str();
    let first = demo_sir();
    let second = SirAnnotation {
        intent: "Reviewed intent".to_owned(),
        confidence: 0.95,
        ..demo_sir()
    };

    // The job is queued while the store holds H1 (version 1)...
    pipeline
        .persist_sir_payload_into_sqlite(
            &store,
            &payload_for(&symbol, &first, SIR_GENERATION_PASS_SCAN),
            None,
        )
        .expect("persist H1");
    let prior_sir = current_sir_identity(&store, symbol_id).expect("identity");
    assert_eq!(
        prior_sir,
        Some(SirIdentity {
            sir_hash: sir_hash(&first),
            sir_version: 1,
            write_generation: 1,
        })
    );

    // ...and while it generates, two injections take the SIR to H2 and back to H1.
    for sir in [&second, &first] {
        pipeline
            .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, sir, "injected"), None)
            .expect("inject");
    }
    let current = current_sir_identity(&store, symbol_id).expect("identity");
    assert_eq!(
        current,
        Some(SirIdentity {
            sir_hash: sir_hash(&first),
            sir_version: 3,
            write_generation: 3,
        }),
        "the same content written again is a new write generation"
    );

    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: SirAnnotation {
            intent: "Generated later, from the older state".to_owned(),
            ..demo_sir()
        },
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir,
        source_hash: symbol.content_hash.clone(),
    };
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(
        matches!(persisted, GenerationPersist::Superseded),
        "a hash match alone must not let the older generation through"
    );
    assert_eq!(
        current_sir_identity(&store, symbol_id).expect("identity"),
        current
    );
    assert_eq!(count_table_rows(workspace, "write_intents"), 0);

    // The same generation against an unchanged store lands.
    let generated = infer::GeneratedSir {
        prior_sir: current,
        ..generated
    };
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(matches!(persisted, GenerationPersist::Persisted(_)));
    assert_eq!(
        store
            .get_sir_meta(symbol_id)
            .expect("meta")
            .expect("meta exists")
            .sir_hash,
        sir_hash(&generated.sir)
    );
}

#[test]
fn persist_successful_generation_sqlite_rolls_back_when_sqlite_done_fails() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn rollback() {}\n");
    store.upsert_symbol(record).expect("upsert symbol");
    let symbol_id = symbol.id.as_str();
    install_sqlite_done_failure_trigger(workspace, symbol_id);

    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: demo_sir(),
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: None,
        source_hash: symbol.content_hash.clone(),
    };

    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("sqlite stage should handle failure");
    assert!(matches!(persisted, GenerationPersist::Failed));
    assert_eq!(count_table_rows(workspace, "sir_history"), 0);
    assert_eq!(count_table_rows(workspace, "sir"), 0);
    assert_eq!(count_table_rows(workspace, "write_intents"), 1);
    assert_eq!(store.read_sir_blob(symbol_id).expect("read sir blob"), None);

    let failed = store.get_failed_intents().expect("load failed intents");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].symbol_id, symbol_id);
    assert_eq!(failed[0].status, WriteIntentStatus::Failed);
    assert!(
        failed[0]
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("sqlite_done blocked for test"))
    );
}

/// An embedding provider that, while embedding, replaces the symbol's SIR through
/// another store connection: an injection landing during the daemon's provider call.
struct InjectingEmbeddingProvider {
    workspace: std::path::PathBuf,
    symbol_id: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl aether_infer::EmbeddingProvider for InjectingEmbeddingProvider {
    async fn embed_text(&self, _text: &str) -> std::result::Result<Vec<f32>, InferError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let store = SqliteStore::open(&self.workspace).expect("open store from the provider");
        store
            .persist_sir_state_atomically_with_source(
                aether_store::SirMetaRecord {
                    id: self.symbol_id.clone(),
                    sir_hash: "hash-injected-meanwhile".to_owned(),
                    sir_version: 1,
                    provider: "manual".to_owned(),
                    model: "manual".to_owned(),
                    generation_pass: "injected".to_owned(),
                    reasoning_trace: None,
                    prompt_hash: None,
                    staleness_score: None,
                    updated_at: 1_700_000_500,
                    sir_status: SIR_STATUS_FRESH.to_owned(),
                    last_error: None,
                    last_attempt_at: 1_700_000_500,
                },
                r#"{"intent":"injected meanwhile","confidence":0.95}"#,
                None,
                None,
                None,
            )
            .expect("the injection lands while the vector is generated");
        Ok(vec![1.0, 0.0])
    }
}

#[test]
fn a_generation_superseded_during_its_embedding_retires_its_intent_without_a_success() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn raced() {}\n");
    store.upsert_symbol(record).expect("upsert symbol");
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = build_write_pipeline_with_embeddings(
        workspace,
        Arc::new(PanicInferenceProvider),
        Some(Arc::new(InjectingEmbeddingProvider {
            workspace: workspace.to_path_buf(),
            symbol_id: symbol.id.clone(),
            calls: Arc::clone(&calls),
        })),
    );
    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: SirAnnotation {
            intent: "Generated by the daemon".to_owned(),
            ..demo_sir()
        },
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: None,
        source_hash: symbol.content_hash.clone(),
    };

    // The leaf commits, then an injection replaces it while its vector is generated.
    // The commit is not a success (no intent id is returned, so the event path does
    // not count it), the intent is retired rather than advanced to vector_done for a
    // leaf the store no longer holds, and no vector for the daemon's SIR is stored.
    let mut out = Vec::new();
    let outcome = pipeline
        .commit_successful_generation(
            &store,
            generated,
            SIR_GENERATION_PASS_SCAN,
            None,
            false,
            &mut out,
        )
        .expect("commit");
    assert!(
        outcome.is_none(),
        "a superseded generation is not a success"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the vector was generated once"
    );
    assert_eq!(
        store
            .read_sir_blob(&symbol.id)
            .expect("read blob")
            .as_deref(),
        Some(r#"{"intent":"injected meanwhile","confidence":0.95}"#),
        "the injection stands"
    );
    assert!(
        store
            .get_incomplete_intents()
            .expect("incomplete intents")
            .is_empty(),
        "the daemon's intent is retired, not left at vector_done or failed"
    );
    assert!(
        pipeline
            .load_symbol_embedding(&symbol.id)
            .expect("load embedding")
            .is_none(),
        "no vector for the replaced SIR is left behind"
    );
}

/// An embedding provider that, while embedding, replaces the symbol's SIR and then
/// restores the very same content through another store connection. The row ends up
/// with the hash the daemon wrote, but as another write: two leaf writes later.
struct RestoringEmbeddingProvider {
    workspace: std::path::PathBuf,
    symbol_id: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl aether_infer::EmbeddingProvider for RestoringEmbeddingProvider {
    async fn embed_text(&self, _text: &str) -> std::result::Result<Vec<f32>, InferError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let store = SqliteStore::open(&self.workspace).expect("open store from the provider");
        let snapshot = store
            .get_sir_meta_with_blob(&self.symbol_id)
            .expect("read the daemon's row")
            .expect("the daemon's write is committed");
        let blob = snapshot.blob.expect("the row holds its JSON");
        store
            .persist_sir_state_atomically(
                aether_store::SirMetaRecord {
                    sir_hash: "hash-replaced-meanwhile".to_owned(),
                    ..snapshot.meta.clone()
                },
                r#"{"intent":"replaced meanwhile","confidence":0.5}"#,
                None,
                None,
            )
            .expect("the replacement lands while the vector is generated");
        store
            .persist_sir_state_atomically(snapshot.meta, &blob, None, None)
            .expect("the restore lands while the vector is generated");
        Ok(vec![1.0, 0.0])
    }
}

#[test]
fn a_generation_replaced_and_restored_during_its_embedding_is_still_superseded() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn restored() {}\n");
    store.upsert_symbol(record).expect("upsert symbol");
    let calls = Arc::new(AtomicUsize::new(0));
    let pipeline = build_write_pipeline_with_embeddings(
        workspace,
        Arc::new(PanicInferenceProvider),
        Some(Arc::new(RestoringEmbeddingProvider {
            workspace: workspace.to_path_buf(),
            symbol_id: symbol.id.clone(),
            calls: Arc::clone(&calls),
        })),
    );
    let sir = SirAnnotation {
        intent: "Generated by the daemon".to_owned(),
        ..demo_sir()
    };
    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: sir.clone(),
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: None,
        source_hash: symbol.content_hash.clone(),
    };

    // The row carries the daemon's hash again when the vector comes back, but two
    // writes have landed since the daemon's own: currency is the whole identity, so
    // the commit is superseded like any other, retires its intent and stores no vector.
    let mut out = Vec::new();
    let outcome = pipeline
        .commit_successful_generation(
            &store,
            generated,
            SIR_GENERATION_PASS_SCAN,
            None,
            false,
            &mut out,
        )
        .expect("commit");
    assert!(
        outcome.is_none(),
        "a write replaced and restored underneath the daemon is not its success"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the vector was generated once"
    );
    let current = current_sir_identity(&store, &symbol.id)
        .expect("identity")
        .expect("the restored row");
    assert_eq!(
        current.sir_hash,
        sir_hash(&sir),
        "the restore carries the daemon's hash"
    );
    assert_eq!(
        current.write_generation, 3,
        "but it is the third write of the row"
    );
    assert!(
        store
            .get_incomplete_intents()
            .expect("incomplete intents")
            .is_empty(),
        "the daemon's intent is retired"
    );
    assert!(
        pipeline
            .load_symbol_embedding(&symbol.id)
            .expect("load embedding")
            .is_none(),
        "no vector is stored for a write the daemon no longer owns"
    );
}

#[test]
fn rewriting_the_same_sir_content_is_a_new_write_generation() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let symbol_id = "sym-forced";
    store
        .upsert_symbol(demo_symbol(symbol_id, "demo::forced"))
        .expect("upsert symbol");
    let source = "fn forced() {}\n";
    let symbol = demo_type_symbol(
        symbol_id,
        "forced",
        "demo::forced",
        "src/lib.rs",
        SymbolKind::Function,
        source,
    );
    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(workspace.join("src/lib.rs"), source).expect("write source");
    let sir = demo_sir();

    // The job is queued against the first write of this content...
    pipeline
        .persist_sir_payload_into_sqlite(
            &store,
            &payload_for(&symbol, &sir, SIR_GENERATION_PASS_SCAN),
            None,
        )
        .expect("persist");
    let observed = current_sir_identity(&store, symbol_id).expect("identity");
    assert_eq!(
        observed,
        Some(SirIdentity {
            sir_hash: sir_hash(&sir),
            sir_version: 1,
            write_generation: 1,
        })
    );

    // ...then a forced injection writes the same canonical content again: same hash,
    // same history version, but a write of its own with its own provenance.
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, &sir, "injected"), None)
        .expect("forced injection");
    let current = current_sir_identity(&store, symbol_id).expect("identity");
    assert_eq!(
        current,
        Some(SirIdentity {
            sir_hash: sir_hash(&sir),
            sir_version: 1,
            write_generation: 2,
        })
    );
    assert_ne!(current, observed);

    // The job planned against the first write is superseded by the second.
    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: SirAnnotation {
            intent: "Generated from the older state".to_owned(),
            ..demo_sir()
        },
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: observed,
        source_hash: content_hash(source),
    };
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(matches!(persisted, GenerationPersist::Superseded));
    let meta = store
        .get_sir_meta(symbol_id)
        .expect("meta")
        .expect("meta exists");
    assert_eq!(
        meta.generation_pass, "injected",
        "the forced injection's provenance stands"
    );
}
