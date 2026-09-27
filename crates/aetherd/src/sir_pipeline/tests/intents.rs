//! Write intents: payload round trips, recognizing an intent's own committed write, and the
//! pending and committed replay paths.

use super::*;

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

    // The source hash rides along when known and reads back as unknown otherwise.
    assert_eq!(parsed.source_hash, None);
    payload.source_hash = Some("source-1".to_owned());
    let json = payload.to_json_string().expect("serialize");
    let parsed = UpsertSirIntentPayload::from_json_str(&json).expect("parse");
    assert_eq!(parsed.source_hash.as_deref(), Some("source-1"));
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
fn a_committed_write_is_recognized_across_edge_derived_fields() {
    use super::super::intents::is_own_committed_write;

    let committed = SirAnnotation {
        dependencies: vec!["Helper".to_owned(), "Record".to_owned()],
        method_dependencies: Some(HashMap::from([(
            "load".to_owned(),
            vec!["Helper".to_owned()],
        )])),
        ..demo_sir()
    };
    // Edges recorded since the commit change today's recomputation of the derived
    // fields, not the write itself.
    let recomputed = SirAnnotation {
        dependencies: vec![
            "Helper".to_owned(),
            "Record".to_owned(),
            "StoreError".to_owned(),
        ],
        method_dependencies: Some(HashMap::from([
            (
                "load".to_owned(),
                vec!["Helper".to_owned(), "StoreError".to_owned()],
            ),
            ("save".to_owned(), vec!["Record".to_owned()]),
        ])),
        ..demo_sir()
    };
    assert!(is_own_committed_write(&committed, &recomputed));
    // Any other difference is another writer's SIR.
    let replaced = SirAnnotation {
        intent: "Reviewed by hand".to_owned(),
        ..recomputed.clone()
    };
    assert!(!is_own_committed_write(&committed, &replaced));
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
fn a_pending_intent_is_replayed_only_while_the_symbol_still_has_the_text_it_describes() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let older = "fn replayed() {}\n";
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", older);
    store.upsert_symbol(record).expect("upsert symbol");
    let symbol_id = symbol.id.clone();
    let intent_for = |intent_id: &str, payload: &UpsertSirIntentPayload| WriteIntent {
        intent_id: intent_id.to_owned(),
        symbol_id: symbol_id.clone(),
        file_path: symbol.file_path.clone(),
        operation: IntentOperation::UpsertSir,
        status: WriteIntentStatus::Pending,
        payload_json: Some(payload.to_json_string().expect("payload json")),
        created_at: 1_700_000_000,
        completed_at: None,
        error_message: None,
    };

    // Planned for the text on disk: the replay writes the leaf and records that text.
    let mut payload = payload_for(&symbol, &demo_sir(), SIR_GENERATION_PASS_SCAN);
    payload.prior_sir = PriorSir::Absent;
    payload.source_hash = Some(symbol.content_hash.clone());
    let current = intent_for("intent-current-text", &payload);
    store.create_write_intent(&current).expect("create intent");
    pipeline
        .replay_upsert_sir_intent(&store, &current, &payload, false)
        .expect("replay");
    assert_eq!(
        store.read_sir_blob(&symbol_id).expect("blob").as_deref(),
        Some(canonicalize_sir_json(&payload.sir).as_str())
    );
    assert_eq!(
        store.get_sir_source_hash(&symbol_id).expect("source hash"),
        Some(symbol.content_hash.clone()),
        "the payload's source hash is recorded with the leaf"
    );

    // Planned for that text too, but the body was edited before the replay (same id):
    // the intent is retired without writing, and the daemon's job for the new text is
    // left to describe it.
    let mut stale = payload_for(
        &symbol,
        &SirAnnotation {
            intent: "Describes the older text".to_owned(),
            ..demo_sir()
        },
        SIR_GENERATION_PASS_SCAN,
    );
    stale.prior_sir =
        PriorSir::recorded(current_sir_identity(&store, &symbol_id).expect("identity"));
    stale.source_hash = Some(symbol.content_hash.clone());
    let edited = parsed_symbols(workspace, "src/lib.rs", "fn replayed() {\n    2\n}\n")
        .into_iter()
        .next()
        .expect("the edited source declares the symbol");
    assert_eq!(edited.id, symbol_id);
    let pending = intent_for("intent-older-text", &stale);
    store.create_write_intent(&pending).expect("create intent");
    pipeline
        .replay_upsert_sir_intent(&store, &pending, &stale, false)
        .expect("replay");
    assert_eq!(
        store
            .get_intent("intent-older-text")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::Complete,
        "an intent for text the symbol no longer has is retired"
    );
    assert_eq!(
        store.read_sir_blob(&symbol_id).expect("blob").as_deref(),
        Some(canonicalize_sir_json(&payload.sir).as_str()),
        "the older description is not written"
    );
}

#[test]
fn a_pending_intent_for_a_removed_symbol_is_retired_without_writing() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    // The symbol is on disk but no longer in the index (removed since the intent was
    // planned); the intent is a legacy one: no prior SIR recorded, no source hash.
    let (symbol, _record) = parsed_symbol(workspace, "src/lib.rs", "fn gone() {}\n");
    let mut payload = payload_for(&symbol, &demo_sir(), SIR_GENERATION_PASS_SCAN);
    payload.prior_sir = PriorSir::Unrecorded;
    payload.source_hash = None;
    let pending = WriteIntent {
        intent_id: "intent-removed-symbol".to_owned(),
        symbol_id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        operation: IntentOperation::UpsertSir,
        status: WriteIntentStatus::Pending,
        payload_json: Some(payload.to_json_string().expect("payload json")),
        created_at: 1_700_000_000,
        completed_at: None,
        error_message: None,
    };
    store.create_write_intent(&pending).expect("create intent");

    pipeline
        .replay_upsert_sir_intent(&store, &pending, &payload, false)
        .expect("replay");
    assert_eq!(
        store
            .get_intent("intent-removed-symbol")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::Complete,
        "an intent for a symbol the index no longer holds is retired"
    );
    assert!(
        store.get_sir_meta(&symbol.id).expect("meta").is_none(),
        "no orphan SIR row is written for the removed symbol"
    );
}

#[test]
fn a_committed_intent_whose_stored_sir_cannot_be_parsed_fails_the_replay_instead_of_retiring_it() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = build_write_pipeline(workspace, Arc::new(PanicInferenceProvider));
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn damaged() {}\n");
    store.upsert_symbol(record).expect("upsert symbol");
    let payload = payload_for(&symbol, &demo_sir(), SIR_GENERATION_PASS_SCAN);
    // The intent's SQLite stage landed, but the stored blob is damaged.
    store
        .persist_sir_state_atomically(
            aether_store::SirMetaRecord {
                id: symbol.id.clone(),
                sir_hash: "hash-damaged".to_owned(),
                sir_version: 1,
                provider: "test_provider".to_owned(),
                model: "test_model".to_owned(),
                generation_pass: SIR_GENERATION_PASS_SCAN.to_owned(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_000,
                sir_status: SIR_STATUS_FRESH.to_owned(),
                last_error: None,
                last_attempt_at: 1_700_000_000,
            },
            "{not json",
            None,
            None,
        )
        .expect("persist damaged blob");
    let done = WriteIntent {
        intent_id: "intent-damaged-blob".to_owned(),
        symbol_id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        operation: IntentOperation::UpsertSir,
        status: WriteIntentStatus::SqliteDone,
        payload_json: Some(payload.to_json_string().expect("payload json")),
        created_at: 1_700_000_000,
        completed_at: None,
        error_message: None,
    };
    store.create_write_intent(&done).expect("create intent");

    // Damage is reported by name and the intent stays where it was, not retired as if
    // a newer writer had replaced its SIR.
    let err = pipeline
        .replay_upsert_sir_intent(&store, &done, &payload, false)
        .expect_err("a stored SIR that cannot be parsed fails the replay");
    assert!(
        format!("{err:#}").contains(&format!("failed to parse the stored SIR of {}", symbol.id)),
        "unexpected error: {err:#}"
    );
    assert_eq!(
        store
            .get_intent("intent-damaged-blob")
            .expect("intent")
            .expect("intent exists")
            .status,
        WriteIntentStatus::SqliteDone,
        "the intent is not retired over a blob nobody wrote as a SIR"
    );
}
