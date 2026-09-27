//! Leaf persistence tests: generation persist guards, write-intent payloads and
//! replay, and the sqlite rollback.

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
fn intent_payload_round_trips_the_prior_sir_record() {
    let symbol = demo_type_symbol(
        "sym-prior",
        "prior",
        "demo::prior",
        "src/lib.rs",
        SymbolKind::Function,
        "fn prior() {}\n",
    );
    let mut payload = payload_for(&symbol, &demo_sir(), SIR_GENERATION_PASS_SCAN);

    // Unrecorded: the key is left out, so the intent reads back as unrecorded.
    let json = payload.to_json_string().expect("serialize");
    assert!(!json.contains("prior_sir"));
    let parsed = UpsertSirIntentPayload::from_json_str(&json).expect("parse");
    assert_eq!(parsed.prior_sir, PriorSir::Unrecorded);
    assert!(parsed.prior_sir.still_holds(None));
    assert!(parsed.prior_sir.still_holds(Some(&SirIdentity {
        sir_hash: "h".to_owned(),
        sir_version: 7,
        write_generation: 7,
    })));

    // Absent: null, and only a store without a SIR still holds it.
    payload.prior_sir = PriorSir::Absent;
    let json = payload.to_json_string().expect("serialize");
    assert!(json.contains("\"prior_sir\":null"));
    let parsed = UpsertSirIntentPayload::from_json_str(&json).expect("parse");
    assert_eq!(parsed.prior_sir, PriorSir::Absent);
    assert!(parsed.prior_sir.still_holds(None));
    assert!(!parsed.prior_sir.still_holds(Some(&SirIdentity {
        sir_hash: "h".to_owned(),
        sir_version: 1,
        write_generation: 1,
    })));

    // Present: hash and version both have to match.
    let identity = SirIdentity {
        sir_hash: "h1".to_owned(),
        sir_version: 2,
        write_generation: 2,
    };
    payload.prior_sir = PriorSir::Present(identity.clone());
    let json = payload.to_json_string().expect("serialize");
    let parsed = UpsertSirIntentPayload::from_json_str(&json).expect("parse");
    assert_eq!(parsed.prior_sir, PriorSir::Present(identity.clone()));
    assert!(parsed.prior_sir.still_holds(Some(&identity)));
    assert!(!parsed.prior_sir.still_holds(None));
    assert!(!parsed.prior_sir.still_holds(Some(&SirIdentity {
        sir_hash: "h1".to_owned(),
        sir_version: 3,
        write_generation: 3,
    })));
    assert!(!parsed.prior_sir.still_holds(Some(&SirIdentity {
        sir_hash: "h2".to_owned(),
        sir_version: 2,
        write_generation: 2,
    })));

    // Intents written before the record existed carry no key at all.
    let legacy = UpsertSirIntentPayload::from_json_str(
        json.replace(
            ",\"prior_sir\":{\"sir_hash\":\"h1\",\"sir_version\":2,\"write_generation\":2}",
            "",
        )
        .as_str(),
    )
    .expect("parse legacy payload");
    assert_eq!(legacy.prior_sir, PriorSir::Unrecorded);
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
fn replay_retires_an_intent_whose_sir_moved_on() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let symbol_id = "sym-replay";
    store
        .upsert_symbol(demo_symbol(symbol_id, "demo::replay"))
        .expect("upsert symbol");
    let symbol = demo_type_symbol(
        symbol_id,
        "replay",
        "demo::replay",
        "src/lib.rs",
        SymbolKind::Function,
        "fn replay() {}\n",
    );
    let generated = demo_sir();
    let reviewed = SirAnnotation {
        intent: "Reviewed by hand".to_owned(),
        confidence: 0.97,
        ..demo_sir()
    };
    let reviewed_json = canonicalize_sir_json(&reviewed);
    let intent_for =
        |intent_id: &str, status: WriteIntentStatus, payload_json: String| WriteIntent {
            intent_id: intent_id.to_owned(),
            symbol_id: symbol_id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            operation: IntentOperation::UpsertSir,
            status,
            payload_json: Some(payload_json),
            created_at: 1_700_000_000,
            completed_at: None,
            error_message: None,
        };

    // A pending intent planned while the symbol had no SIR: an injection landed
    // before the replay, so the intent is retired and the injected SIR stands.
    let mut payload = payload_for(&symbol, &generated, SIR_GENERATION_PASS_SCAN);
    payload.prior_sir = PriorSir::Absent;
    let pending = intent_for(
        "intent-pending",
        WriteIntentStatus::Pending,
        payload.to_json_string().expect("payload json"),
    );
    store.create_write_intent(&pending).expect("create intent");
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, &reviewed, "injected"), None)
        .expect("inject");
    pipeline
        .replay_upsert_sir_intent(&store, &pending, &payload, false)
        .expect("replay pending intent");
    assert_eq!(
        store.read_sir_blob(symbol_id).expect("blob").as_deref(),
        Some(reviewed_json.as_str()),
        "the replay must not restore the generated SIR over the injected one"
    );
    assert_eq!(
        store
            .get_intent("intent-pending")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::Complete
    );

    // An intent whose sqlite stage landed, then was replaced by an injection, is
    // superseded too: the newer blob is left alone rather than refreshed back.
    let payload = payload_for(&symbol, &generated, SIR_GENERATION_PASS_SCAN);
    let done = intent_for(
        "intent-done",
        WriteIntentStatus::Pending,
        payload.to_json_string().expect("payload json"),
    );
    store.create_write_intent(&done).expect("create intent");
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload, Some("intent-done"))
        .expect("sqlite stage");
    assert_eq!(
        store
            .get_intent("intent-done")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::SqliteDone
    );
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, &reviewed, "injected"), None)
        .expect("inject");
    let done = WriteIntent {
        status: WriteIntentStatus::SqliteDone,
        ..done
    };
    pipeline
        .replay_upsert_sir_intent(&store, &done, &payload, false)
        .expect("replay sqlite_done intent");
    assert_eq!(
        store.read_sir_blob(symbol_id).expect("blob").as_deref(),
        Some(reviewed_json.as_str())
    );
    assert_eq!(
        store
            .get_intent("intent-done")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::Complete
    );

    // A pending intent planned against the SIR the store still holds is replayed.
    let mut payload = payload_for(
        &symbol,
        &SirAnnotation {
            intent: "Regenerated".to_owned(),
            ..demo_sir()
        },
        SIR_GENERATION_PASS_SCAN,
    );
    payload.prior_sir =
        PriorSir::recorded(current_sir_identity(&store, symbol_id).expect("identity"));
    let pending = intent_for(
        "intent-current",
        WriteIntentStatus::Pending,
        payload.to_json_string().expect("payload json"),
    );
    store.create_write_intent(&pending).expect("create intent");
    pipeline
        .replay_upsert_sir_intent(&store, &pending, &payload, false)
        .expect("replay current intent");
    assert_eq!(
        store.read_sir_blob(symbol_id).expect("blob").as_deref(),
        Some(canonicalize_sir_json(&payload.sir).as_str())
    );

    // An intent whose sqlite stage committed and whose embedding stage then failed
    // keeps its stage when the failure is recorded: the replay resumes from it against
    // the intent's own committed SIR and completes, instead of re-planning the write
    // against a store that now holds it and retiring the intent as superseded.
    let mut payload = payload_for(
        &symbol,
        &SirAnnotation {
            intent: "Committed, then the embedding failed".to_owned(),
            ..demo_sir()
        },
        SIR_GENERATION_PASS_SCAN,
    );
    payload.prior_sir =
        PriorSir::recorded(current_sir_identity(&store, symbol_id).expect("identity"));
    let resumed = intent_for(
        "intent-resumed",
        WriteIntentStatus::Pending,
        payload.to_json_string().expect("payload json"),
    );
    store.create_write_intent(&resumed).expect("create intent");
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload, Some("intent-resumed"))
        .expect("sqlite stage");
    store
        .mark_intent_failed("intent-resumed", "embedding provider unavailable")
        .expect("record the failed stage");
    let resumed = store
        .get_intent("intent-resumed")
        .expect("intent")
        .expect("intent exists");
    assert_eq!(resumed.status, WriteIntentStatus::SqliteDone);
    pipeline
        .replay_upsert_sir_intent(&store, &resumed, &payload, false)
        .expect("replay the interrupted intent");
    assert_eq!(
        store
            .get_intent("intent-resumed")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::Complete
    );
    assert_eq!(
        store.read_sir_blob(symbol_id).expect("blob").as_deref(),
        Some(canonicalize_sir_json(&payload.sir).as_str()),
        "the intent's own committed SIR stands"
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

#[test]
fn build_job_records_the_hash_of_the_text_it_read_for_the_prompt() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let snapshot_source = "fn late() {}\n";
    let symbol = demo_type_symbol(
        "sym-late",
        "late",
        "demo::late",
        "src/lib.rs",
        SymbolKind::Function,
        snapshot_source,
    );
    // The file was edited after the snapshot the symbol came from: the recorded range
    // now covers other text, so the job is refused rather than prompting on that text.
    let edited_source = "fn late() { edited }\n";
    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(workspace.join("src/lib.rs"), edited_source).expect("write source");
    let mut edited_symbol = symbol.clone();
    edited_symbol.range.end_byte = Some(edited_source.len());
    edited_symbol.range.end.column = edited_source.trim_end().len() + 1;
    let err = build_job(workspace, edited_symbol, None, Some(8))
        .err()
        .expect("a symbol whose text changed since indexing must be refused");
    assert!(
        format!("{err:#}").contains("changed since it was indexed"),
        "unexpected error: {err:#}"
    );

    // With the file as the snapshot saw it, the job carries the hash of the full text
    // it read (the parser's hash), even when the prompt text is truncated.
    fs::write(workspace.join("src/lib.rs"), snapshot_source).expect("restore source");
    let job = build_job(workspace, symbol.clone(), None, Some(8)).expect("build job");
    assert_eq!(job.symbol_text, "fn late(");
    assert_eq!(job.source_hash, content_hash(snapshot_source));
    assert_eq!(job.source_hash, symbol.content_hash);
}

#[test]
fn a_prompt_override_binds_the_job_to_the_baseline_it_was_built_from() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let source = "fn deep() {}\n";
    fs::create_dir_all(workspace.join("src")).expect("create src");
    fs::write(workspace.join("src/lib.rs"), source).expect("write source");
    let symbol = demo_type_symbol(
        "sym-deep",
        "deep",
        "demo::deep",
        "src/lib.rs",
        SymbolKind::Function,
        source,
    );

    // `regenerate --deep` builds its enrichment from this SIR...
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, &demo_sir(), "scan"), None)
        .expect("persist baseline");
    let baseline = current_sir_identity(&store, "sym-deep").expect("identity");
    assert!(baseline.is_some());

    // ...and another writer replaces it before the job is queued.
    let reviewed = SirAnnotation {
        intent: "Reviewed by hand".to_owned(),
        confidence: 0.97,
        ..demo_sir()
    };
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, &reviewed, "injected"), None)
        .expect("persist replacement");
    let replacement = current_sir_identity(&store, "sym-deep").expect("identity");
    assert_ne!(replacement, baseline);

    let prepare = |prior_sir: PriorSir| {
        let overrides = HashMap::from([(
            symbol.id.clone(),
            SirPromptOverride {
                prompt: "deep prompt built from the baseline".to_owned(),
                deep_mode: false,
                prior_sir,
            },
        )]);
        let mut out = Vec::<u8>::new();
        let prepared = pipeline
            .prepare_candidate_jobs(
                &store,
                "src/lib.rs",
                vec![(symbol.clone(), false)],
                true,
                false,
                &mut out,
                None,
                Some(&overrides),
            )
            .expect("prepare jobs");
        assert_eq!(prepared.jobs.len(), 1);
        prepared.jobs.into_iter().next().expect("one job").prior_sir
    };

    // A recorded baseline binds the job to the SIR its prompt describes, so the
    // persist step will find the replacement and drop the result as superseded...
    assert_eq!(prepare(PriorSir::recorded(baseline.clone())), baseline);
    // ...while an unrecorded one binds to whatever the row holds when queued.
    assert_eq!(prepare(PriorSir::Unrecorded), replacement);
}

#[test]
fn a_failed_generation_marks_only_the_sir_it_started_from_stale() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let symbol_id = "sym-failed";
    store
        .upsert_symbol(demo_symbol(symbol_id, "demo::failed"))
        .expect("upsert symbol");
    let symbol = demo_type_symbol(
        symbol_id,
        "failed",
        "demo::failed",
        "src/lib.rs",
        SymbolKind::Function,
        "fn failed() {}\n",
    );
    let placeholder = demo_sir();
    pipeline
        .persist_sir_payload_into_sqlite(
            &store,
            &payload_for(&symbol, &placeholder, SIR_GENERATION_PASS_SCAN),
            None,
        )
        .expect("persist placeholder");
    let started_from = current_sir_identity(&store, symbol_id).expect("identity");

    // While the job ran, an injection replaced the placeholder with a reviewed SIR.
    let reviewed = SirAnnotation {
        intent: "Reviewed by hand".to_owned(),
        confidence: 0.97,
        ..demo_sir()
    };
    pipeline
        .persist_sir_payload_into_sqlite(&store, &payload_for(&symbol, &reviewed, "injected"), None)
        .expect("inject");

    // The job's failure must not mark the reviewed SIR stale or rewrite its provenance.
    pipeline
        .handle_failed_generation(
            &store,
            infer::FailedSirGeneration {
                symbol: symbol.clone(),
                error_message: "provider timed out".to_owned(),
                prior_sir: started_from,
            },
            SIR_GENERATION_PASS_SCAN,
            false,
            &mut std::io::sink(),
        )
        .expect("handle failure");
    let meta = store
        .get_sir_meta(symbol_id)
        .expect("meta")
        .expect("meta exists");
    assert_eq!(meta.sir_status, SIR_STATUS_FRESH);
    assert_eq!(meta.generation_pass, "injected");
    assert_eq!(meta.last_error, None);

    // A failure against the SIR the store still holds is recorded on it.
    pipeline
        .handle_failed_generation(
            &store,
            infer::FailedSirGeneration {
                symbol,
                error_message: "provider timed out".to_owned(),
                prior_sir: current_sir_identity(&store, symbol_id).expect("identity"),
            },
            SIR_GENERATION_PASS_SCAN,
            false,
            &mut std::io::sink(),
        )
        .expect("handle failure");
    let meta = store
        .get_sir_meta(symbol_id)
        .expect("meta")
        .expect("meta exists");
    assert_eq!(meta.sir_status, SIR_STATUS_STALE);
    assert_eq!(meta.last_error.as_deref(), Some("provider timed out"));
    assert_eq!(
        meta.sir_hash,
        sir_hash(&reviewed),
        "the reviewed SIR itself stays"
    );
}

#[test]
fn persist_successful_generation_sqlite_skips_a_sir_whose_source_changed() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let original = "fn edited() {}\n";
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", original);
    store.upsert_symbol(record).expect("upsert symbol");
    let symbol_id = symbol.id.as_str();

    // The job read the original body; while it ran, the body was edited (same
    // signature, so the same symbol id) and the stored SIR did not change.
    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: demo_sir(),
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: None,
        source_hash: symbol.content_hash.clone(),
    };
    fs::write(workspace.join("src/lib.rs"), "fn edited() { changed }\n").expect("edit source");
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(
        matches!(persisted, GenerationPersist::Superseded),
        "a result generated from a body the symbol no longer has must not land"
    );
    assert_eq!(store.read_sir_blob(symbol_id).expect("read blob"), None);
    assert_eq!(count_table_rows(workspace, "write_intents"), 0);

    // With the body as the job read it, moved down by an edit elsewhere in the file,
    // the symbol is found by id at its new position and the result lands: the range
    // the job recorded no longer covers it, and that must not read as a change.
    fs::write(
        workspace.join("src/lib.rs"),
        format!("fn other() {{}}\n\n{original}"),
    )
    .expect("shift source");
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(matches!(persisted, GenerationPersist::Persisted(_)));
    assert!(store.read_sir_blob(symbol_id).expect("read blob").is_some());
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
