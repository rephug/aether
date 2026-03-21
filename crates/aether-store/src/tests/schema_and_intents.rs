use super::*;

#[test]
fn migration_v6_renames_legacy_generation_pass_values_to_scan() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    let db_path = workspace.join(".aether/meta.sqlite");
    let conn = Connection::open(&db_path).expect("open sqlite db");
    conn.execute_batch(
            r#"
            CREATE TABLE sir (
                id TEXT PRIMARY KEY,
                sir_hash TEXT NOT NULL,
                sir_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                generation_pass TEXT DEFAULT 'single',
                updated_at INTEGER NOT NULL,
                sir_status TEXT NOT NULL DEFAULT 'fresh',
                last_error TEXT,
                last_attempt_at INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO sir (id, sir_hash, sir_version, provider, model, generation_pass, updated_at, sir_status, last_error, last_attempt_at)
            VALUES
                ('sym-triage', 'hash-a', 1, 'legacy', 'legacy', 'triage', 1, 'fresh', NULL, 1),
                ('sym-single', 'hash-b', 1, 'legacy', 'legacy', 'single', 1, 'fresh', NULL, 1);
            PRAGMA user_version = 5;
            "#,
        )
        .expect("seed legacy schema");
    drop(conn);

    let store = SqliteStore::open(workspace).expect("open migrated store");
    let triage_meta = store
        .get_sir_meta("sym-triage")
        .expect("load triage meta")
        .expect("triage meta exists");
    let single_meta = store
        .get_sir_meta("sym-single")
        .expect("load single meta")
        .expect("single meta exists");

    assert_eq!(triage_meta.generation_pass, "scan");
    assert_eq!(single_meta.generation_pass, "scan");
    assert_eq!(
        store.get_schema_version().expect("schema version").version,
        14
    );
}

#[test]
fn migration_v7_expands_symbol_edge_kinds() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        r#"
            CREATE TABLE symbol_edges (
                source_id TEXT NOT NULL,
                target_qualified_name TEXT NOT NULL,
                edge_kind TEXT NOT NULL CHECK (edge_kind IN ('calls', 'depends_on')),
                file_path TEXT NOT NULL,
                PRIMARY KEY (source_id, target_qualified_name, edge_kind)
            );
            INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path)
            VALUES ('sym-a', 'beta', 'calls', 'src/lib.rs');
            PRAGMA user_version = 6;
            "#,
    )
    .expect("seed v6 symbol_edges schema");

    run_migrations(&conn).expect("run migrations");

    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("query migrated version");
    assert_eq!(version, 14);

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbol_edges", [], |row| row.get(0))
        .expect("count migrated edges");
    assert_eq!(count, 1);

    conn.execute(
        r#"
            INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path)
            VALUES (?1, ?2, ?3, ?4)
            "#,
        params!["sym-b", "TargetType", "type_ref", "src/lib.rs"],
    )
    .expect("insert type_ref edge after migration");
}

#[test]
fn open_store_migrates_legacy_sir_table_with_stale_defaults() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let aether_dir = workspace.join(".aether");
    let sir_dir = aether_dir.join("sir");
    fs::create_dir_all(&sir_dir).expect("create legacy aether dirs");

    let sqlite_path = aether_dir.join("meta.sqlite");
    let conn = Connection::open(&sqlite_path).expect("open legacy sqlite");
    conn.execute_batch(
        r#"
            CREATE TABLE IF NOT EXISTS symbols (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                signature_fingerprint TEXT NOT NULL,
                last_seen_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sir (
                id TEXT PRIMARY KEY,
                sir_hash TEXT NOT NULL,
                sir_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
    )
    .expect("create legacy schema");

    conn.execute(
            "INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params!["legacy-sym", "legacy-hash", 1i64, "mock", "mock", 1_700_000_500i64],
        )
        .expect("insert legacy sir row");
    drop(conn);

    let store = SqliteStore::open(workspace).expect("open migrated store");
    let migrated = store
        .get_sir_meta("legacy-sym")
        .expect("load migrated row")
        .expect("row exists");

    assert_eq!(migrated.sir_status, "fresh");
    assert_eq!(migrated.last_error, None);
    assert_eq!(migrated.last_attempt_at, migrated.updated_at);
    assert!(
        store
            .list_sir_history("legacy-sym")
            .expect("load history for legacy row without sir_json")
            .is_empty()
    );

    let embedding_lookup = store
        .search_symbols_semantic(&[1.0, 0.0], "mock", "mock-64d", 10)
        .expect("semantic search on migrated schema");
    assert!(embedding_lookup.is_empty());
}

#[test]
fn open_store_migrates_legacy_symbols_table_with_access_columns() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let aether_dir = workspace.join(".aether");
    let sir_dir = aether_dir.join("sir");
    fs::create_dir_all(&sir_dir).expect("create legacy aether dirs");

    let sqlite_path = aether_dir.join("meta.sqlite");
    let conn = Connection::open(&sqlite_path).expect("open legacy sqlite");
    conn.execute_batch(
        r#"
            CREATE TABLE IF NOT EXISTS symbols (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                signature_fingerprint TEXT NOT NULL,
                last_seen_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sir (
                id TEXT PRIMARY KEY,
                sir_hash TEXT NOT NULL,
                sir_version INTEGER NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                sir_json TEXT
            );
            "#,
    )
    .expect("create legacy schema");
    conn.execute(
            "INSERT INTO symbols (id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params!["legacy-sym", "src/lib.rs", "rust", "function", "demo::legacy", "sig", 1_700_000_000i64],
        )
        .expect("insert legacy symbol");
    drop(conn);

    let store = SqliteStore::open(workspace).expect("open migrated store");
    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT access_count, last_accessed_at FROM symbols WHERE id = ?1")
        .expect("prepare migrated symbol lookup");
    let access = stmt
        .query_row(params!["legacy-sym"], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .expect("query migrated symbol access fields");
    assert_eq!(access.0, 0);
    assert_eq!(access.1, None);
}

#[test]
fn run_migrations_sets_user_version_and_is_idempotent() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");

    run_migrations(&conn).expect("run migrations once");
    let first_version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("query first user_version");
    assert_eq!(first_version, 14);

    run_migrations(&conn).expect("run migrations twice");
    let second_version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("query second user_version");
    assert_eq!(second_version, 14);
}

#[test]
fn schema_version_table_is_populated() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let schema = store.get_schema_version().expect("get schema version");
    assert_eq!(schema.component, "core");
    assert_eq!(schema.version, 14);
    assert!(schema.migrated_at > 0);
}

#[test]
fn migration_v14_creates_sir_quality_table() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");

    run_migrations(&conn).expect("run migrations");

    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("query migrated version");
    assert_eq!(version, 14);

    let columns = conn
        .prepare("PRAGMA table_info(sir_quality)")
        .expect("prepare sir_quality table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query sir_quality columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect sir_quality columns");
    assert_eq!(
        columns,
        vec![
            "sir_id",
            "specificity",
            "behavioral_depth",
            "error_coverage",
            "length_score",
            "composite_quality",
            "confidence_percentile",
            "normalized_quality",
            "computed_at",
        ]
    );
}

#[test]
fn migration_v9_adds_prompt_hash_and_fingerprint_history() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    run_migrations(&conn).expect("run migrations");

    let prompt_hash_columns = conn
        .prepare("PRAGMA table_info(sir)")
        .expect("prepare sir table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query sir columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect sir columns");
    assert!(
        prompt_hash_columns
            .iter()
            .any(|column| column == "prompt_hash")
    );

    let fingerprint_columns = conn
        .prepare("PRAGMA table_info(sir_fingerprint_history)")
        .expect("prepare fingerprint table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query fingerprint columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect fingerprint columns");
    assert!(
        fingerprint_columns
            .iter()
            .any(|column| column == "symbol_id")
    );
    assert!(
        fingerprint_columns
            .iter()
            .any(|column| column == "prompt_hash")
    );
    assert!(
        fingerprint_columns
            .iter()
            .any(|column| column == "delta_sem")
    );
}

#[test]
fn migration_v10_adds_staleness_score_to_sir() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    run_migrations(&conn).expect("run migrations");

    let sir_columns = conn
        .prepare("PRAGMA table_info(sir)")
        .expect("prepare sir table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query sir columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect sir columns");
    assert!(sir_columns.iter().any(|column| column == "staleness_score"));
}

#[test]
fn write_intent_status_and_operation_round_trip() {
    let statuses = [
        WriteIntentStatus::Pending,
        WriteIntentStatus::SqliteDone,
        WriteIntentStatus::VectorDone,
        WriteIntentStatus::GraphDone,
        WriteIntentStatus::Complete,
        WriteIntentStatus::Failed,
    ];
    for status in statuses {
        let text = status.to_string();
        let parsed = WriteIntentStatus::from_str(text.as_str()).expect("parse status");
        assert_eq!(parsed, status);
    }

    let operations = [
        IntentOperation::UpsertSir,
        IntentOperation::DeleteSymbol,
        IntentOperation::UpdateEdges,
    ];
    for operation in operations {
        let text = operation.to_string();
        let parsed = IntentOperation::from_str(text.as_str()).expect("parse operation");
        assert_eq!(parsed, operation);
    }
}

#[test]
fn write_intent_crud_updates_complete_and_failed() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let intent = write_intent_record("intent-1", WriteIntentStatus::Pending);
    store
        .create_write_intent(&intent)
        .expect("create write intent");
    store
        .update_intent_status(&intent.intent_id, WriteIntentStatus::SqliteDone)
        .expect("update intent status");
    store
        .update_intent_status(&intent.intent_id, WriteIntentStatus::VectorDone)
        .expect("update intent status");
    store
        .mark_intent_complete(&intent.intent_id)
        .expect("mark complete");

    let loaded = store
        .get_intent(&intent.intent_id)
        .expect("get intent")
        .expect("intent exists");
    assert_eq!(loaded.status, WriteIntentStatus::Complete);
    assert!(loaded.completed_at.is_some());
    assert_eq!(loaded.error_message, None);

    let failed = write_intent_record("intent-2", WriteIntentStatus::Pending);
    store
        .create_write_intent(&failed)
        .expect("create failed write intent");
    store
        .mark_intent_failed(&failed.intent_id, "vector write failed")
        .expect("mark failed");

    let loaded_failed = store
        .get_intent(&failed.intent_id)
        .expect("get failed intent")
        .expect("failed intent exists");
    assert_eq!(loaded_failed.status, WriteIntentStatus::Failed);
    assert_eq!(
        loaded_failed.error_message.as_deref(),
        Some("vector write failed")
    );
}

#[test]
fn get_incomplete_intents_excludes_complete_and_failed() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .create_write_intent(&write_intent_record(
            "intent-pending",
            WriteIntentStatus::Pending,
        ))
        .expect("insert pending");
    store
        .create_write_intent(&write_intent_record(
            "intent-sqlite",
            WriteIntentStatus::SqliteDone,
        ))
        .expect("insert sqlite_done");
    store
        .create_write_intent(&write_intent_record(
            "intent-complete",
            WriteIntentStatus::Complete,
        ))
        .expect("insert complete");
    store
        .create_write_intent(&write_intent_record(
            "intent-failed",
            WriteIntentStatus::Failed,
        ))
        .expect("insert failed");

    let incomplete = store
        .get_incomplete_intents()
        .expect("list incomplete intents");
    let ids = incomplete
        .iter()
        .map(|intent| intent.intent_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["intent-pending", "intent-sqlite"]);
}

#[test]
fn prune_completed_intents_removes_only_old_completed_rows() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .create_write_intent(&write_intent_record(
            "intent-old-complete",
            WriteIntentStatus::Complete,
        ))
        .expect("insert old complete");
    store
        .create_write_intent(&write_intent_record(
            "intent-new-complete",
            WriteIntentStatus::Complete,
        ))
        .expect("insert new complete");
    store
        .create_write_intent(&write_intent_record(
            "intent-pending",
            WriteIntentStatus::Pending,
        ))
        .expect("insert pending");

    let conn = store.conn.lock().unwrap();
    conn.execute(
            "UPDATE write_intents SET completed_at = unixepoch() - 1_000_000 WHERE intent_id = 'intent-old-complete'",
            [],
        )
        .expect("set old completed_at");
    conn.execute(
            "UPDATE write_intents SET completed_at = unixepoch() - 10 WHERE intent_id = 'intent-new-complete'",
            [],
        )
        .expect("set new completed_at");
    drop(conn);

    let deleted = store
        .prune_completed_intents(604_800)
        .expect("prune completed intents");
    assert_eq!(deleted, 1);

    assert!(
        store
            .get_intent("intent-old-complete")
            .expect("get old")
            .is_none()
    );
    assert!(
        store
            .get_intent("intent-new-complete")
            .expect("get new")
            .is_some()
    );
    assert!(
        store
            .get_intent("intent-pending")
            .expect("get pending")
            .is_some()
    );
}

#[test]
fn migration_from_v2_to_v3_adds_write_intents_table() {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute("PRAGMA user_version = 2", [])
        .expect("set v2 schema version");

    run_migrations(&conn).expect("run migrations");
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("query user_version");
    assert_eq!(version, 14);

    let task_history_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='task_context_history' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .expect("query sqlite_master")
        .is_some();
    assert!(task_history_exists);

    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='write_intents' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .expect("query sqlite_master")
        .is_some();
    assert!(exists);

    let requests_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sir_requests' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .expect("query sqlite_master")
        .is_some();
    assert!(requests_exists);
}

#[test]
fn task_context_history_round_trips_in_newest_first_order() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .insert_task_context_history(&TaskContextHistoryRecord {
            task_description: "older task".to_owned(),
            branch_name: Some("feature/older".to_owned()),
            resolved_symbol_ids: "[\"sym-a\"]".to_owned(),
            resolved_file_paths: "[\"src/lib.rs\"]".to_owned(),
            total_symbols: 1,
            budget_used: 1200,
            budget_max: 32_000,
            created_at: 1_700_000_000,
        })
        .expect("insert older task history");
    store
        .insert_task_context_history(&TaskContextHistoryRecord {
            task_description: "newer task".to_owned(),
            branch_name: None,
            resolved_symbol_ids: "[\"sym-b\",\"sym-c\"]".to_owned(),
            resolved_file_paths: "[\"src/main.rs\"]".to_owned(),
            total_symbols: 2,
            budget_used: 2400,
            budget_max: 32_000,
            created_at: 1_700_000_100,
        })
        .expect("insert newer task history");

    let history = store
        .list_recent_task_history(10)
        .expect("list recent history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].task_description, "newer task");
    assert_eq!(history[0].branch_name, None);
    assert_eq!(history[1].task_description, "older task");
    assert_eq!(history[1].branch_name.as_deref(), Some("feature/older"));
}

#[test]
fn threshold_calibration_round_trip_persists_latest_value() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_threshold_calibration(ThresholdCalibrationRecord {
            language: "rust".to_owned(),
            threshold: 0.72,
            sample_size: 123,
            provider: "mock".to_owned(),
            model: "mock-64d".to_owned(),
            calibrated_at: "2026-02-19T00:00:00Z".to_owned(),
        })
        .expect("upsert threshold");
    store
        .upsert_threshold_calibration(ThresholdCalibrationRecord {
            language: "rust".to_owned(),
            threshold: 0.74,
            sample_size: 456,
            provider: "mock".to_owned(),
            model: "mock-64d".to_owned(),
            calibrated_at: "2026-02-19T00:01:00Z".to_owned(),
        })
        .expect("upsert threshold update");

    let rust = store
        .get_threshold_calibration("rust")
        .expect("get threshold")
        .expect("threshold exists");
    assert_eq!(rust.threshold, 0.74);
    assert_eq!(rust.sample_size, 456);
    assert_eq!(rust.provider, "mock");

    let all = store
        .list_threshold_calibrations()
        .expect("list threshold calibrations");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].language, "rust");
}

#[test]
fn list_embeddings_for_provider_model_returns_language_context() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_symbol(SymbolRecord {
            id: "sym-rust".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "demo::run".to_owned(),
            signature_fingerprint: "sig-rust".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert rust symbol");
    store
        .upsert_symbol(SymbolRecord {
            id: "sym-py".to_owned(),
            file_path: "src/jobs.py".to_owned(),
            language: "python".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "jobs.run".to_owned(),
            signature_fingerprint: "sig-py".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert python symbol");
    store
        .upsert_symbol_embedding(SymbolEmbeddingRecord {
            symbol_id: "sym-rust".to_owned(),
            sir_hash: "hash-rust".to_owned(),
            provider: "mock".to_owned(),
            model: "mock-64d".to_owned(),
            embedding: vec![1.0, 0.0],
            updated_at: 1_700_000_100,
        })
        .expect("upsert rust embedding");
    store
        .upsert_symbol_embedding(SymbolEmbeddingRecord {
            symbol_id: "sym-py".to_owned(),
            sir_hash: "hash-py".to_owned(),
            provider: "mock".to_owned(),
            model: "mock-64d".to_owned(),
            embedding: vec![0.0, 1.0],
            updated_at: 1_700_000_101,
        })
        .expect("upsert python embedding");

    let rows = store
        .list_embeddings_for_provider_model("mock", "mock-64d")
        .expect("list embeddings");
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|row| row.symbol_id == "sym-rust" && row.language == "rust")
    );
    assert!(
        rows.iter()
            .any(|row| row.symbol_id == "sym-py" && row.language == "python")
    );
}
