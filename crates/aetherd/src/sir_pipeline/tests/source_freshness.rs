use super::*;

fn fixture() -> (tempfile::TempDir, SqliteStore, SirPipeline, Symbol) {
    let temp = tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join(".aether")).expect("config directory");
    fs::write(
        temp.path().join(".aether/config.toml"),
        "[storage]\ngraph_backend = \"sqlite\"\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n",
    )
    .expect("write config");
    let (symbol, record) = parsed_symbol(
        temp.path(),
        "src/lib.rs",
        "pub fn compute(value: i32) -> i32 { value + 1 }\n",
    );
    let store = SqliteStore::open(temp.path()).expect("store");
    store.upsert_symbol(record).expect("catalog symbol");
    let pipeline = build_write_pipeline(
        temp.path(),
        Arc::new(FixedInferenceProvider {
            sir: SirAnnotation {
                intent: "New provider explanation".to_owned(),
                ..demo_sir()
            },
        }),
    );
    (temp, store, pipeline, symbol)
}

fn seed(
    store: &SqliteStore,
    symbol: &Symbol,
    source_hash: Option<&str>,
    status: &str,
    updated_at: i64,
) {
    let sir = SirAnnotation {
        intent: "Manual explanation".to_owned(),
        ..demo_sir()
    };
    store
        .persist_sir_state_atomically_with_source(
            SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: sir_hash(&sir),
                sir_version: 1,
                provider: "manual".to_owned(),
                model: "manual".to_owned(),
                generation_pass: "injected".to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at,
                sir_status: status.to_owned(),
                last_error: None,
                last_attempt_at: updated_at,
            },
            &canonicalize_sir_json(&sir),
            None,
            None,
            source_hash,
        )
        .expect("seed manual SIR");
}

#[test]
fn matching_source_skips_even_when_timestamp_would_regenerate() {
    let (temp, store, pipeline, symbol) = fixture();
    let modified_ms = source_modified_unix_millis(&temp.path().join(&symbol.file_path))
        .expect("source modification time");
    seed(
        &store,
        &symbol,
        Some(&symbol.content_hash),
        "fresh",
        modified_ms / 1_000,
    );
    assert!(
        pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("skip decision")
    );
}

#[test]
fn mismatched_source_regenerates_even_when_timestamp_would_skip() {
    let (_temp, store, pipeline, symbol) = fixture();
    seed(
        &store,
        &symbol,
        Some("older-source"),
        "fresh",
        unix_timestamp_secs() + 3_600,
    );
    assert!(
        !pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("skip decision")
    );
}

#[test]
fn missing_source_hash_preserves_legacy_timestamp_behavior() {
    let (_temp, store, pipeline, symbol) = fixture();
    seed(
        &store,
        &symbol,
        None,
        "fresh",
        unix_timestamp_secs() + 3_600,
    );
    assert!(
        pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("legacy recent SIR")
    );
    seed(&store, &symbol, None, "fresh", 0);
    assert!(
        !pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("legacy old SIR")
    );
}

#[test]
fn matching_hash_does_not_hide_stale_or_missing_content() {
    let (_temp, store, pipeline, symbol) = fixture();
    assert!(
        !pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("missing SIR")
    );
    seed(
        &store,
        &symbol,
        Some(&symbol.content_hash),
        "stale",
        unix_timestamp_secs() + 3_600,
    );
    assert!(
        !pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("stale SIR")
    );
    seed(
        &store,
        &symbol,
        Some(&symbol.content_hash),
        "fresh",
        unix_timestamp_secs() + 3_600,
    );
    store.write_sir_blob(&symbol.id, "").expect("empty blob");
    assert!(
        !pipeline
            .should_skip_sir_generation(&store, &symbol)
            .expect("empty SIR")
    );
}

#[test]
fn explicit_force_still_regenerates_a_matching_source() {
    let (_temp, store, pipeline, symbol) = fixture();
    seed(&store, &symbol, Some(&symbol.content_hash), "fresh", 0);
    let event = SymbolChangeEvent {
        file_path: symbol.file_path.clone(),
        language: symbol.language,
        added: vec![symbol.clone()],
        removed: Vec::new(),
        updated: Vec::new(),
    };
    let skipped = pipeline
        .process_event_with_priority_and_pass(
            &store,
            &event,
            false,
            false,
            &mut std::io::sink(),
            None,
            SIR_GENERATION_PASS_SCAN,
        )
        .expect("ordinary skip");
    assert_eq!(skipped.success_count, 0);
    assert_eq!(skipped.generation_failure_count, 0);
    assert_eq!(skipped.unresolved_leaf_count, 0);
    let forced = pipeline
        .process_event_with_priority_and_pass(
            &store,
            &event,
            true,
            false,
            &mut std::io::sink(),
            None,
            SIR_GENERATION_PASS_SCAN,
        )
        .expect("forced regeneration");
    assert_eq!(forced.success_count, 1);
    assert_eq!(forced.generation_failure_count, 0);
    assert_eq!(forced.unresolved_leaf_count, 0);
    let blob = store
        .read_sir_blob(&symbol.id)
        .expect("read SIR")
        .expect("SIR exists");
    let sir: SirAnnotation = serde_json::from_str(&blob).expect("valid SIR");
    assert_eq!(sir.intent, "New provider explanation");
}

struct UnavailableProvider;

#[async_trait::async_trait]
impl InferenceProvider for UnavailableProvider {
    fn provider_name(&self) -> String {
        "unavailable".to_owned()
    }
    fn model_name(&self) -> String {
        "unavailable".to_owned()
    }
    async fn generate_sir(
        &self,
        _symbol_text: &str,
        _context: &SirContext,
    ) -> std::result::Result<SirAnnotation, InferError> {
        Err(InferError::InvalidResponse(
            "fixture provider outage".to_owned(),
        ))
    }
}

struct FailedOutput;

impl std::io::Write for FailedOutput {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("fixture output failure"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn secondary_error_keeps_provider_failure_classification() {
    let (temp, store, _pipeline, symbol) = fixture();
    let pipeline = build_write_pipeline(temp.path(), Arc::new(UnavailableProvider));
    let event = SymbolChangeEvent {
        file_path: symbol.file_path.clone(),
        language: symbol.language,
        added: vec![symbol],
        removed: Vec::new(),
        updated: Vec::new(),
    };
    let error = pipeline
        .process_event_with_priority_and_pass(
            &store,
            &event,
            false,
            true,
            &mut FailedOutput,
            None,
            SIR_GENERATION_PASS_SCAN,
        )
        .expect_err("writing the provider-failure report fails");
    assert!(
        error.is::<ProviderGenerationFailure>(),
        "provider recovery must survive its reporting error: {error:#}"
    );
}

#[test]
fn candidate_build_failure_retains_unresolved_leaf_debt() {
    let (temp, store, pipeline, symbol) = fixture();
    fs::remove_file(temp.path().join(&symbol.file_path)).expect("remove source");
    let prepared = pipeline
        .prepare_candidate_jobs(
            &store,
            &symbol.file_path,
            vec![(symbol.clone(), false)],
            true,
            false,
            &mut Vec::new(),
            None,
            None,
            None,
        )
        .expect("preparation reports individual build failures");
    assert!(prepared.jobs.is_empty());
    assert_eq!(prepared.skipped_existing, 0);
    assert_eq!(prepared.unresolved_leaf_count, 1);
    let event = SymbolChangeEvent {
        file_path: symbol.file_path.clone(),
        language: symbol.language,
        added: vec![symbol],
        removed: Vec::new(),
        updated: Vec::new(),
    };
    let stats = pipeline
        .process_event_with_priority_and_pass(
            &store,
            &event,
            false,
            false,
            &mut std::io::sink(),
            None,
            SIR_GENERATION_PASS_SCAN,
        )
        .expect("a swallowed source-read failure returns explicit processing stats");
    assert_eq!(stats.success_count, 0);
    assert_eq!(stats.generation_failure_count, 0);
    assert_eq!(stats.unresolved_leaf_count, 1);
}

#[test]
fn a_swallowed_persistence_failure_returns_explicit_unresolved_stats() {
    let (temp, store, pipeline, symbol) = fixture();
    let connection =
        Connection::open(temp.path().join(".aether/meta.sqlite")).expect("open fixture database");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_test_intent BEFORE INSERT ON write_intents BEGIN
         SELECT RAISE(FAIL, 'fixture persistence outage'); END;",
        )
        .expect("install temporary write failure");
    let event = SymbolChangeEvent {
        file_path: symbol.file_path.clone(),
        language: symbol.language,
        added: vec![symbol.clone()],
        removed: Vec::new(),
        updated: Vec::new(),
    };
    let stats = pipeline
        .process_event_with_priority_and_pass(
            &store,
            &event,
            false,
            false,
            &mut std::io::sink(),
            None,
            SIR_GENERATION_PASS_SCAN,
        )
        .expect("a caught persistence failure is reported through processing stats");
    assert_eq!(stats.success_count, 0);
    assert_eq!(stats.failure_count, 1);
    assert_eq!(stats.generation_failure_count, 0);
    assert_eq!(stats.unresolved_leaf_count, 1);
    assert!(
        store
            .read_sir_blob(&symbol.id)
            .expect("read missing leaf")
            .is_none()
    );
    connection
        .execute_batch("DROP TRIGGER reject_test_intent;")
        .expect("restore fixture persistence");
}
