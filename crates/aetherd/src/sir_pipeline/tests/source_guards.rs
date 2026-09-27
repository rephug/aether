//! Source binding: jobs record the text they read, persist and failure markers are guarded
//! by the symbol's current source and existence.

use super::*;

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
fn a_job_for_newer_text_replaces_a_sir_bound_to_older_text() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let older = "fn late() {}\n";
    let (symbol_x, record) = parsed_symbol(workspace, "src/lib.rs", older);
    store.upsert_symbol(record).expect("upsert symbol");
    let symbol_id = symbol_x.id.clone();
    let leaf = |hash: &str| aether_store::SirMetaRecord {
        id: symbol_id.clone(),
        sir_hash: hash.to_owned(),
        sir_version: 1,
        provider: "manual".to_owned(),
        model: "manual".to_owned(),
        generation_pass: "injected".to_owned(),
        reasoning_trace: None,
        prompt_hash: None,
        staleness_score: None,
        updated_at: 1_700_000_000,
        sir_status: SIR_STATUS_FRESH.to_owned(),
        last_error: None,
        last_attempt_at: 1_700_000_000,
    };
    // The store holds a SIR of the older text, and a job is queued against it after
    // the body is edited (same signature, same id).
    store
        .persist_sir_state_atomically_with_source(
            leaf("hash-older"),
            r#"{"intent":"older","confidence":0.9}"#,
            None,
            None,
            Some(symbol_x.content_hash.as_str()),
        )
        .expect("persist the SIR of the older text");
    let newer = "fn late() {\n    2\n}\n";
    let symbol_y = parsed_symbols(workspace, "src/lib.rs", newer)
        .into_iter()
        .next()
        .expect("the edited source declares the symbol");
    assert_eq!(symbol_y.id, symbol_id, "the edit keeps the id");
    assert_ne!(symbol_y.content_hash, symbol_x.content_hash);
    let queued_against = current_sir_identity(&store, &symbol_id).expect("identity");
    let generated = infer::GeneratedSir {
        symbol: symbol_y.clone(),
        sir: SirAnnotation {
            intent: "Describes the newer text".to_owned(),
            ..demo_sir()
        },
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: queued_against,
        source_hash: symbol_y.content_hash.clone(),
    };

    // While the job ran, an injection bound to the OLDER text committed (it passed its
    // source check just before the edit): the identity moved on, but the stored SIR
    // records the older text, so the job's result, for the current text, replaces it
    // rather than yielding and leaving a SIR of the old body `fresh` for good.
    store
        .persist_sir_state_atomically_with_source(
            leaf("hash-stale-injection"),
            r#"{"intent":"stale injection of the older text","confidence":0.95}"#,
            None,
            None,
            Some(symbol_x.content_hash.as_str()),
        )
        .expect("stale injection lands");
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(
        matches!(persisted, GenerationPersist::Persisted(_)),
        "a SIR bound to older text does not supersede the job for the current text"
    );
    let stored: SirAnnotation = serde_json::from_str(
        &store
            .read_sir_blob(&symbol_id)
            .expect("read blob")
            .expect("blob"),
    )
    .expect("parse blob");
    assert_eq!(stored.intent, "Describes the newer text");
    assert_eq!(
        store.get_sir_source_hash(&symbol_id).expect("source hash"),
        Some(symbol_y.content_hash.clone()),
        "the daemon's write records the text it was generated from"
    );

    // An injection bound to the SAME text the job read, or one that records no text,
    // still supersedes the job as before.
    for (label, source) in [
        ("same text", Some(symbol_y.content_hash.as_str())),
        ("unrecorded text", None),
    ] {
        let generated = infer::GeneratedSir {
            prior_sir: current_sir_identity(&store, &symbol_id).expect("identity"),
            ..generated.clone()
        };
        store
            .persist_sir_state_atomically_with_source(
                leaf(&format!("hash-{}", label.replace(' ', "-"))),
                r#"{"intent":"another writer","confidence":0.95}"#,
                None,
                None,
                source,
            )
            .expect("competing write lands");
        let persisted = pipeline
            .persist_successful_generation_sqlite(
                &store,
                &generated,
                SIR_GENERATION_PASS_SCAN,
                None,
            )
            .expect("persist");
        assert!(
            matches!(persisted, GenerationPersist::Superseded),
            "a write bound to the {label} supersedes the job"
        );
    }
}

#[test]
fn a_source_that_cannot_be_read_fails_the_persist_instead_of_superseding_it() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn unreadable() {}\n");
    store.upsert_symbol(record).expect("upsert symbol");
    let generated = infer::GeneratedSir {
        symbol: symbol.clone(),
        sir: demo_sir(),
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        reasoning_trace: None,
        prior_sir: None,
        source_hash: symbol.content_hash.clone(),
    };

    // The file exists but cannot be read (a directory stands in its place). That says
    // nothing about the symbol's text, so the result is neither dropped as superseded
    // nor written: the persist fails naming the file, and the job stays to be retried.
    fs::remove_file(workspace.join("src/lib.rs")).expect("remove file");
    fs::create_dir(workspace.join("src/lib.rs")).expect("directory in the file's place");
    let err = match pipeline.persist_successful_generation_sqlite(
        &store,
        &generated,
        SIR_GENERATION_PASS_SCAN,
        None,
    ) {
        Err(err) => err,
        Ok(_) => panic!("an unreadable source must fail the persist"),
    };
    assert!(
        format!("{err:#}").contains("failed to read src/lib.rs"),
        "unexpected error: {err:#}"
    );
    assert!(
        store.get_sir_meta(&symbol.id).expect("meta").is_none(),
        "the failed persist wrote nothing"
    );

    // A file that is gone is the one case that means the symbol is gone with it: the
    // result is superseded, as for any other body the symbol no longer has.
    fs::remove_dir(workspace.join("src/lib.rs")).expect("remove directory");
    let persisted = pipeline
        .persist_successful_generation_sqlite(&store, &generated, SIR_GENERATION_PASS_SCAN, None)
        .expect("persist");
    assert!(matches!(persisted, GenerationPersist::Superseded));
}

#[test]
fn a_failed_generation_for_a_removed_symbol_writes_no_marker() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let symbol_id = "sym-removed";
    store
        .upsert_symbol(demo_symbol(symbol_id, "demo::removed"))
        .expect("upsert symbol");
    let symbol = demo_type_symbol(
        symbol_id,
        "removed",
        "demo::removed",
        "src/lib.rs",
        SymbolKind::Function,
        "fn removed() {}\n",
    );

    // The job started with no SIR; while it ran, the symbol was removed from the index.
    // "No SIR" before and after must not read as unchanged: a failure marker would
    // recreate an orphan SIR row for an id the index no longer holds.
    store.mark_removed(symbol_id).expect("remove symbol");
    pipeline
        .handle_failed_generation(
            &store,
            infer::FailedSirGeneration {
                symbol,
                error_message: "provider timed out".to_owned(),
                prior_sir: None,
            },
            SIR_GENERATION_PASS_SCAN,
            false,
            &mut std::io::sink(),
        )
        .expect("handle failure");
    assert_eq!(store.get_sir_meta(symbol_id).expect("meta"), None);
    assert_eq!(count_table_rows(workspace, "sir"), 0);
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
