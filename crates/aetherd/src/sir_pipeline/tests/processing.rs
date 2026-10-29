//! Event, bulk-scan and quality-batch processing tests.

use super::*;

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
    assert!(
        store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty()
    );
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
    assert!(
        store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty()
    );
    assert_eq!(
        store
            .count_intents_by_status()
            .expect("count intents")
            .get("complete"),
        Some(&symbols.len())
    );
    for symbol in &symbols {
        assert!(
            store
                .get_symbol_embedding_meta(symbol.id.as_str())
                .expect("read embedding meta")
                .is_some()
        );
    }

    let rollup_id = synthetic_file_sir_id("rust", "src/lib.rs");
    assert!(
        store
            .read_sir_blob(rollup_id.as_str())
            .expect("read rollup blob")
            .is_some()
    );
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
    assert!(
        store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty()
    );

    let rollup_id = synthetic_file_sir_id("rust", "src/lib.rs");
    assert!(
        store
            .read_sir_blob(rollup_id.as_str())
            .expect("read rollup blob")
            .is_some()
    );
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
    let pipeline = build_write_pipeline(
        workspace,
        Arc::new(FixedInferenceProvider { sir: demo_sir() }),
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
    assert!(
        store
            .get_incomplete_intents()
            .expect("load incomplete intents")
            .is_empty()
    );
    assert_eq!(
        store
            .count_intents_by_status()
            .expect("count intents")
            .get("complete"),
        Some(&symbols.len())
    );
    for symbol in &symbols {
        assert!(
            store
                .get_symbol_embedding_meta(symbol.id.as_str())
                .expect("read embedding meta")
                .is_some()
        );
    }

    let rollup_id = synthetic_file_sir_id("rust", "src/lib.rs");
    assert!(
        store
            .read_sir_blob(rollup_id.as_str())
            .expect("read rollup blob")
            .is_some()
    );
}

#[test]
fn process_event_resolves_python_calls_through_package_reexports() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    for (path, source) in [
        ("pkg/__init__.py", "from .impl import f\n"),
        ("pkg/impl.py", "def f():\n    return 1\n"),
        (
            "tests/test_pkg.py",
            "from pkg import f\n\n\ndef test_f():\n    f()\n",
        ),
    ] {
        let full_path = workspace.join(path);
        fs::create_dir_all(full_path.parent().expect("parent")).expect("create dirs");
        fs::write(full_path, source).expect("write source");
    }

    let store = SqliteStore::open(workspace).expect("open store");
    let source = fs::read_to_string(workspace.join("tests/test_pkg.py")).expect("read source");
    let test_f = SymbolExtractor::new()
        .expect("symbol extractor")
        .extract_with_edges_from_path(Path::new("tests/test_pkg.py"), &source)
        .expect("extract source")
        .symbols
        .into_iter()
        .find(|symbol| symbol.qualified_name == "tests.test_pkg::test_f")
        .expect("test symbol");
    store
        .upsert_symbol(demo_symbol_record_with_kind(
            test_f.id.as_str(),
            test_f.qualified_name.as_str(),
            test_f.kind.as_str(),
            test_f.file_path.as_str(),
        ))
        .expect("upsert symbol");

    let provider = Arc::new(FixedInferenceProvider {
        sir: SirAnnotation {
            intent: "Checks f".to_owned(),
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
        file_path: "tests/test_pkg.py".to_owned(),
        language: Language::Python,
        added: Vec::new(),
        removed: Vec::new(),
        updated: vec![test_f.clone()],
    };
    pipeline
        .process_event_with_priority_and_pass(
            &store,
            &event,
            true,
            false,
            &mut Vec::new(),
            None,
            SIR_GENERATION_PASS_REGENERATED,
        )
        .expect("process event");

    // The pipeline re-extracts the file's edges as the structural indexer does: through
    // the package's re-export, not as the raw `pkg::f`.
    let targets = store
        .list_symbol_edges_for_source_and_kinds(test_f.id.as_str(), &[EdgeKind::Calls])
        .expect("read edges")
        .into_iter()
        .map(|edge| edge.target_qualified_name)
        .collect::<Vec<_>>();
    assert_eq!(targets, vec!["pkg.impl::f".to_owned()]);
}

#[test]
fn inference_only_no_removal_events_preserve_pending_graph_cleanup() {
    use aether_config::{
        AetherConfig, EmbeddingVectorBackend, GraphBackend, save_workspace_config,
    };
    use aether_store::{RetirementCleanupStore, RetirementProjection};
    for use_compatibility_alias in [false, true] {
        let temp = tempdir().expect("workspace");
        let workspace = temp.path();
        let mut config = AetherConfig::default();
        config.storage.graph_backend = GraphBackend::Surreal;
        config.embeddings.vector_backend = EmbeddingVectorBackend::Sqlite;
        config.embeddings.enabled = false;
        save_workspace_config(workspace, &config).expect("temporary config");
        fs::create_dir(workspace.join("src")).expect("source dir");
        fs::write(
            workspace.join("src/unrelated.rs"),
            "// unrelated empty source\n",
        )
        .expect("source");
        let store = SqliteStore::open(workspace).expect("store");
        store
            .upsert_symbol(demo_symbol_record_with_kind(
                "retired",
                "retired",
                "function",
                "src/old.rs",
            ))
            .expect("old symbol");
        store.mark_removed("retired").expect("retirement");
        let before = store
            .list_pending_retirement_cleanup(10)
            .expect("pending graph");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].projection, RetirementProjection::Graph);
        assert_eq!(before[0].attempts, 0);
        // A file-level edge from an older version must still be refreshed by
        // the compatibility alias, which skips only the external graph.
        store
            .upsert_edges(&[SymbolEdge {
                source_id: "file:src/unrelated.rs".to_owned(),
                target_qualified_name: "StaleDependency".to_owned(),
                edge_kind: EdgeKind::DependsOn,
                file_path: "src/unrelated.rs".to_owned(),
            }])
            .expect("stale local edge");
        assert!(
            !workspace.join(".aether/graph").exists(),
            "fixture opened Surreal"
        );
        let pipeline = SirPipeline::new_with_provider(
            workspace.to_path_buf(),
            1,
            Arc::new(PanicInferenceProvider),
            "panic",
            "panic",
        )
        .expect("inference-only pipeline");
        assert!(
            !workspace.join(".aether/graph").exists(),
            "constructor opened Surreal"
        );
        let pipeline = if use_compatibility_alias {
            pipeline.with_skip_surreal_sync(true)
        } else {
            pipeline.with_skip_graph_sync(true)
        };
        let event = SymbolChangeEvent {
            file_path: "src/unrelated.rs".to_owned(),
            language: Language::Rust,
            added: Vec::new(),
            updated: Vec::new(),
            removed: Vec::new(),
        };
        pipeline
            .process_event(&store, &event, false, false, &mut Vec::new())
            .expect("unrelated event");
        let local_edges = store
            .list_symbol_edges_for_source_and_kinds("file:src/unrelated.rs", &[EdgeKind::DependsOn])
            .expect("local edge refresh");
        assert_eq!(
            local_edges.len(),
            usize::from(!use_compatibility_alias),
            "compatibility alias must refresh SQLite edges; full graph skip must leave them alone"
        );
        assert_eq!(
            store
                .list_pending_retirement_cleanup(10)
                .expect("graph remains pending"),
            before
        );
        assert_eq!(
            store
                .retirement_cleanup_summary(0)
                .expect("visible backlog")
                .pending_graph,
            1
        );
        assert!(
            !workspace.join(".aether/graph").exists(),
            "inference-only processing opened Surreal (compatibility alias: {use_compatibility_alias})"
        );
    }
}
