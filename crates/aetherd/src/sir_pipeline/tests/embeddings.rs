//! Embedding refresh and embeddings-only pass tests: the guarded refresh, the
//! per-symbol lock and the pipeline built around a shared provider.

use super::*;

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
fn embeddings_only_pipeline_reuses_the_provider_and_vector_store_it_is_given() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    store
        .upsert_symbol(demo_symbol_record_with_kind(
            "sym-shared",
            "shared",
            "function",
            "src/lib.rs",
        ))
        .expect("upsert symbol");
    let sir = demo_sir();
    let sir_hash_value = seed_sir(&store, "sym-shared", &sir);
    let canonical_json = canonicalize_sir_json(&sir);

    let calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbeddingProvider {
        calls: calls.clone(),
        batch_calls: Arc::new(AtomicUsize::new(0)),
        batch_sizes: Arc::new(Mutex::new(Vec::new())),
        purposes: Arc::new(Mutex::new(Vec::new())),
    });
    let vector_store: Arc<dyn VectorStore> =
        Arc::new(aether_store::SqliteVectorStore::new(workspace).expect("open vector store"));

    let pipeline = SirPipeline::new_embeddings_only_with(
        workspace.to_path_buf(),
        provider,
        "test_embedding",
        "test-model",
        Some(vector_store.clone()),
    )
    .expect("build pipeline");
    assert!(
        Arc::ptr_eq(pipeline.vector_store(), &vector_store),
        "the pipeline must write through the vector store it was handed, not open another"
    );

    // The one pipeline serves symbol after symbol: each refresh goes through the same
    // provider instance (a local model loads once) and lands in the shared store.
    let mut still_current = || Ok(true);
    let refreshed = pipeline
        .refresh_embedding_if_current(
            "sym-shared",
            &sir_hash_value,
            &canonical_json,
            None,
            &mut still_current,
        )
        .expect("refresh");
    assert!(matches!(refreshed, EmbeddingRefresh::Refreshed { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let meta = pipeline
        .runtime
        .block_on(vector_store.get_embedding_meta("sym-shared"))
        .expect("read meta")
        .expect("vector stored");
    assert_eq!(meta.sir_hash, sir_hash_value);
    assert_eq!(meta.provider, "test_embedding");
    assert_eq!(meta.model, "test-model");
}

#[test]
fn embed_write_lock_is_exclusive_per_symbol_within_a_process() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().to_path_buf();
    let guard = acquire_embed_write_lock(&workspace, "sym-lock").expect("lock");
    assert!(
        workspace.join(".aether/embed-locks").is_dir(),
        "the cross-process lock file lives under .aether/embed-locks"
    );
    // Another symbol is independent.
    let other = acquire_embed_write_lock(&workspace, "sym-other").expect("other lock");
    drop(other);

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let contender_workspace = workspace.clone();
    let contender = std::thread::spawn(move || {
        started_tx.send(()).expect("started");
        let _guard =
            acquire_embed_write_lock(&contender_workspace, "sym-lock").expect("contended lock");
        done_tx.send(()).expect("done");
    });
    started_rx.recv().expect("contender started");
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "the contender must wait while the lock is held"
    );
    drop(guard);
    done_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the contender acquires the lock once it is released");
    contender.join().expect("join");

    // Batches take their locks in sorted order and release them together.
    let guards = acquire_embed_write_locks(&workspace, ["b", "a", "b"]).expect("batch locks");
    assert_eq!(guards.len(), 2);
    drop(guards);
    let again = acquire_embed_write_lock(&workspace, "a").expect("released");
    drop(again);
}

#[test]
fn refresh_embedding_if_current_never_leaves_a_vector_for_a_replaced_sir() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    store
        .upsert_symbol(demo_symbol("sym-guard", "demo::guard"))
        .expect("upsert symbol");
    let (provider, calls) = counting_embedding_provider();
    let pipeline = build_write_pipeline_with_embeddings(
        workspace,
        Arc::new(PanicInferenceProvider),
        Some(Arc::new(provider)),
    );

    // Replaced while the provider ran: the vector is never stored.
    let (script, asked) = scripted_check(vec![true, false]);
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-1", "{}", None, &mut || {
            asked.fetch_add(1, Ordering::SeqCst);
            let mut script = script.lock().expect("script");
            Ok(if script.is_empty() {
                true
            } else {
                script.remove(0)
            })
        })
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Superseded);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "provider ran once");
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    assert!(
        pipeline
            .load_symbol_embedding("sym-guard")
            .expect("load embedding")
            .is_none(),
        "no vector for the replaced SIR"
    );

    // Replaced between the last check and the write: the stored vector is removed.
    let (script, asked) = scripted_check(vec![true, true, false]);
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-2", "{}", None, &mut || {
            asked.fetch_add(1, Ordering::SeqCst);
            let mut script = script.lock().expect("script");
            Ok(if script.is_empty() {
                true
            } else {
                script.remove(0)
            })
        })
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Superseded);
    assert_eq!(asked.load(Ordering::SeqCst), 3);
    assert!(
        pipeline
            .load_symbol_embedding("sym-guard")
            .expect("load embedding")
            .is_none(),
        "the vector written for the replaced SIR was deleted"
    );

    // A writer without the embedding locks stores the newer SIR's vector during the
    // provider call (before this call's write): the conditional write must not
    // overwrite it, and the outcome is Superseded with the newer vector intact.
    let (script, asked) = scripted_check(vec![true, true, false]);
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-2a", "{}", None, &mut || {
            let call = asked.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 2 {
                pipeline
                    .runtime
                    .block_on(
                        pipeline
                            .vector_store
                            .upsert_embedding(SymbolEmbeddingRecord {
                                symbol_id: "sym-guard".to_owned(),
                                sir_hash: "hash-raced".to_owned(),
                                provider: "test_embedding".to_owned(),
                                model: "test-model".to_owned(),
                                embedding: vec![0.0, 1.0],
                                updated_at: 1_700_000_400,
                            }),
                    )
                    .expect("racing writer stores its vector");
            }
            let mut script = script.lock().expect("script");
            Ok(if script.is_empty() {
                true
            } else {
                script.remove(0)
            })
        })
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Superseded);
    assert_eq!(
        asked.load(Ordering::SeqCst),
        2,
        "no post-write check without a write"
    );
    let record = pipeline
        .load_symbol_embedding("sym-guard")
        .expect("load embedding")
        .expect("the racing writer's vector survives");
    assert_eq!(record.sir_hash, "hash-raced");
    pipeline
        .runtime
        .block_on(pipeline.vector_store.delete_embedding("sym-guard"))
        .expect("clear for the next case");

    // Replaced in that window by a writer that already stored the newer SIR's
    // vector (the daemon's index pass takes no embedding lock): only this call's
    // own vector may go, so the newer one survives.
    let (script, asked) = scripted_check(vec![true, true, false]);
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-2b", "{}", None, &mut || {
            let call = asked.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 3 {
                pipeline
                    .runtime
                    .block_on(
                        pipeline
                            .vector_store
                            .upsert_embedding(SymbolEmbeddingRecord {
                                symbol_id: "sym-guard".to_owned(),
                                sir_hash: "hash-newer".to_owned(),
                                provider: "test_embedding".to_owned(),
                                model: "test-model".to_owned(),
                                embedding: vec![0.0, 1.0],
                                updated_at: 1_700_000_500,
                            }),
                    )
                    .expect("newer writer stores its vector");
            }
            let mut script = script.lock().expect("script");
            Ok(if script.is_empty() {
                true
            } else {
                script.remove(0)
            })
        })
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Superseded);
    let record = pipeline
        .load_symbol_embedding("sym-guard")
        .expect("load embedding")
        .expect("the newer writer's vector survives");
    assert_eq!(record.sir_hash, "hash-newer");
    pipeline
        .runtime
        .block_on(pipeline.vector_store.delete_embedding("sym-guard"))
        .expect("clear for the next case");

    // Still current throughout: the vector lands and carries the SIR hash.
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-3", "{}", None, &mut || Ok(true))
        .expect("guarded refresh");
    assert_eq!(
        outcome,
        EmbeddingRefresh::Refreshed {
            provider: "test_embedding".to_owned(),
            model: "test-model".to_owned(),
        }
    );
    let record = pipeline
        .load_symbol_embedding("sym-guard")
        .expect("load embedding")
        .expect("vector stored");
    assert_eq!(record.sir_hash, "hash-3");
    assert_eq!(calls.load(Ordering::SeqCst), 5);

    // Already embedded for this hash and still current: no provider call, kept.
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-3", "{}", None, &mut || Ok(true))
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Unchanged);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert!(
        pipeline
            .load_symbol_embedding("sym-guard")
            .expect("load embedding")
            .is_some()
    );

    // Already embedded for this hash but the SIR has moved on (a repeated injection
    // of an old hash after a concurrent newer one): the stale vector is removed
    // without a provider call.
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-3", "{}", None, &mut || Ok(false))
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Superseded);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert!(
        pipeline
            .load_symbol_embedding("sym-guard")
            .expect("load embedding")
            .is_none(),
        "a vector for a superseded SIR must not stay behind"
    );

    // A writer without the embedding locks stores this very SIR's vector under
    // another provider and model during the provider call (the embeddings-only pass
    // reads its metadata before the lock): the conditional write misses, but this
    // SIR is not superseded and the configured identity still has no vector, so the
    // write is retried against the stored row and lands.
    let (script, asked) = scripted_check(vec![true, true, true]);
    let outcome = pipeline
        .refresh_embedding_if_current("sym-guard", "hash-4", "{}", None, &mut || {
            let call = asked.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 2 {
                pipeline
                    .runtime
                    .block_on(
                        pipeline
                            .vector_store
                            .upsert_embedding(SymbolEmbeddingRecord {
                                symbol_id: "sym-guard".to_owned(),
                                sir_hash: "hash-4".to_owned(),
                                provider: "other-provider".to_owned(),
                                model: "other-model".to_owned(),
                                embedding: vec![0.0, 1.0],
                                updated_at: 1_700_000_600,
                            }),
                    )
                    .expect("writer with another identity stores its vector");
            }
            let mut script = script.lock().expect("script");
            Ok(if script.is_empty() {
                true
            } else {
                script.remove(0)
            })
        })
        .expect("guarded refresh");
    assert_eq!(
        outcome,
        EmbeddingRefresh::Refreshed {
            provider: "test_embedding".to_owned(),
            model: "test-model".to_owned(),
        },
        "a same-SIR vector under another identity is not this pass's vector"
    );
    let record = pipeline
        .load_symbol_embedding("sym-guard")
        .expect("load embedding")
        .expect("vector stored");
    assert_eq!(
        (
            record.sir_hash.as_str(),
            record.provider.as_str(),
            record.model.as_str()
        ),
        ("hash-4", "test_embedding", "test-model")
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        6,
        "the provider ran once for the retry"
    );
}

#[test]
fn a_prefetched_vector_identity_is_re_read_under_the_lock_before_it_counts_as_current() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    store
        .upsert_symbol(demo_symbol("sym-prefetched", "demo::prefetched"))
        .expect("upsert symbol");
    let sir_hash_value = seed_sir(&store, "sym-prefetched", &demo_sir());
    let (provider, calls) = counting_embedding_provider();
    let pipeline = build_write_pipeline_with_embeddings(
        workspace,
        Arc::new(PanicInferenceProvider),
        Some(Arc::new(provider)),
    );

    // The embeddings-only pass prefetches every symbol's vector metadata before it takes
    // any symbol's lock. This prefetched record says the configured identity already
    // holds a vector for the SIR, but by the time the lock is held the store has none
    // (another process moved or removed it): the stale record must not make the refresh
    // report the vector unchanged and leave the configured identity without one.
    let prefetched = VectorEmbeddingMetaRecord {
        symbol_id: "sym-prefetched".to_owned(),
        sir_hash: sir_hash_value.clone(),
        provider: "test_embedding".to_owned(),
        model: "test-model".to_owned(),
        embedding_dim: 2,
        updated_at: 1_700_000_100,
    };
    let outcome = pipeline
        .refresh_embedding_if_current(
            "sym-prefetched",
            &sir_hash_value,
            "{}",
            Some(&prefetched),
            &mut || Ok(true),
        )
        .expect("guarded refresh");
    assert!(
        matches!(outcome, EmbeddingRefresh::Refreshed { .. }),
        "the metadata is re-read under the lock: {outcome:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the vector is generated");
    let stored = pipeline
        .load_symbol_embedding("sym-prefetched")
        .expect("load embedding")
        .expect("a vector for the configured identity");
    assert_eq!(stored.sir_hash, sir_hash_value);
    assert_eq!(
        (stored.provider.as_str(), stored.model.as_str()),
        ("test_embedding", "test-model")
    );

    // With the vector in place, the same prefetched record is confirmed by the re-read
    // and the refresh is a no-op.
    let outcome = pipeline
        .refresh_embedding_if_current(
            "sym-prefetched",
            &sir_hash_value,
            "{}",
            Some(&prefetched),
            &mut || Ok(true),
        )
        .expect("guarded refresh");
    assert_eq!(outcome, EmbeddingRefresh::Unchanged);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no second provider call");
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
