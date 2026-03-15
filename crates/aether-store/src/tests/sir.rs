use super::*;

#[test]
fn read_sir_blob_prefers_sqlite_when_mirror_is_missing() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    store
        .write_sir_blob("sym-1", "{\"intent\":\"db-primary\"}")
        .expect("write blob");

    let mirror_path = workspace.join(".aether/sir/sym-1.json");
    fs::remove_file(&mirror_path).expect("remove mirror");

    let loaded = store.read_sir_blob("sym-1").expect("read from sqlite");
    assert_eq!(loaded.as_deref(), Some("{\"intent\":\"db-primary\"}"));

    drop(store);

    let reopened = SqliteStore::open(workspace).expect("reopen store");
    let reopened_loaded = reopened.read_sir_blob("sym-1").expect("read after reopen");
    assert_eq!(
        reopened_loaded.as_deref(),
        Some("{\"intent\":\"db-primary\"}")
    );
}

#[test]
fn read_sir_blob_backfills_sqlite_from_mirror_without_overwriting_meta() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    let meta = SirMetaRecord {
        id: "sym-legacy".to_owned(),
        sir_hash: "legacy-hash".to_owned(),
        sir_version: 3,
        provider: "legacy-provider".to_owned(),
        model: "legacy-model".to_owned(),
        generation_pass: "scan".to_owned(),
        prompt_hash: Some("legacy-source|legacy-neighbor|legacy-config".to_owned()),
        staleness_score: None,
        updated_at: 1_700_111_222,
        sir_status: "fresh".to_owned(),
        last_error: None,
        last_attempt_at: 1_700_111_222,
    };
    store
        .upsert_sir_meta(meta.clone())
        .expect("upsert legacy metadata");

    let mirror_path = workspace.join(".aether/sir/sym-legacy.json");
    fs::write(&mirror_path, "{\"intent\":\"from-mirror\"}").expect("write mirror");

    let first_read = store.read_sir_blob("sym-legacy").expect("first read");
    assert_eq!(first_read.as_deref(), Some("{\"intent\":\"from-mirror\"}"));

    fs::remove_file(&mirror_path).expect("remove mirror");

    let second_read = store.read_sir_blob("sym-legacy").expect("second read");
    assert_eq!(second_read.as_deref(), Some("{\"intent\":\"from-mirror\"}"));

    let meta_after = store
        .get_sir_meta("sym-legacy")
        .expect("read metadata after backfill");
    assert_eq!(meta_after, Some(meta));
}

#[test]
fn get_sir_meta_defaults_generation_pass_to_scan_when_null() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    let conn = store.conn.lock().expect("lock sqlite conn");
    conn.execute(
        r#"
            INSERT INTO sir (
                id, sir_hash, sir_version, provider, model, generation_pass, prompt_hash,
                updated_at, sir_status, last_error, last_attempt_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10)
            "#,
        params![
            "legacy-null-pass",
            "hash-null",
            1_i64,
            "legacy-provider",
            "legacy-model",
            Option::<String>::None,
            1_700_000_500_i64,
            "fresh",
            Option::<String>::None,
            1_700_000_500_i64
        ],
    )
    .expect("insert row with null generation_pass");
    drop(conn);

    let meta = store
        .get_sir_meta("legacy-null-pass")
        .expect("read migrated metadata")
        .expect("metadata should exist");
    assert_eq!(meta.generation_pass, "scan");
    assert_eq!(meta.prompt_hash, None);
}

#[test]
fn sir_meta_round_trips_prompt_hash_and_fingerprint_rows() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    let meta = SirMetaRecord {
        prompt_hash: Some("src123|nbr456|cfg789".to_owned()),
        ..sir_meta_record()
    };
    store
        .upsert_sir_meta(meta.clone())
        .expect("upsert sir meta");

    let loaded = store
        .get_sir_meta(meta.id.as_str())
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(loaded.prompt_hash, meta.prompt_hash);

    store
        .insert_sir_fingerprint_history(&SirFingerprintHistoryRecord {
            symbol_id: meta.id.clone(),
            timestamp: 1_700_000_123,
            prompt_hash: "src123|nbr456|cfg789".to_owned(),
            prompt_hash_previous: Some("src111|nbr222|cfg333".to_owned()),
            trigger: "batch_scan".to_owned(),
            source_changed: true,
            neighbor_changed: false,
            config_changed: true,
            generation_model: Some("gemini-3.1-flash-lite-preview".to_owned()),
            generation_pass: Some("scan".to_owned()),
            delta_sem: Some(0.12),
        })
        .expect("insert fingerprint row");

    let rows = store
        .list_sir_fingerprint_history(meta.id.as_str())
        .expect("list fingerprint rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].prompt_hash, "src123|nbr456|cfg789");
    assert!(rows[0].source_changed);
    assert!(!rows[0].neighbor_changed);
    assert!(rows[0].config_changed);
}

#[test]
fn sir_history_records_are_ordered_and_persist_after_reopen() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    let first = store
        .record_sir_version_if_changed(
            "sym-history",
            "hash-1",
            "mock",
            "mock",
            "{\"intent\":\"v1\"}",
            1_700_222_100,
            Some("1111111111111111111111111111111111111111"),
        )
        .expect("insert history v1");
    assert!(first.changed);
    assert_eq!(first.version, 1);

    let duplicate = store
        .record_sir_version_if_changed(
            "sym-history",
            "hash-1",
            "mock",
            "mock",
            "{\"intent\":\"v1\"}",
            1_700_222_101,
            Some("1111111111111111111111111111111111111111"),
        )
        .expect("dedupe by hash");
    assert!(!duplicate.changed);
    assert_eq!(duplicate.version, 1);
    assert_eq!(duplicate.updated_at, 1_700_222_101);

    let second = store
        .record_sir_version_if_changed(
            "sym-history",
            "hash-2",
            "mock",
            "mock",
            "{\"intent\":\"v2\"}",
            1_700_222_200,
            Some("2222222222222222222222222222222222222222"),
        )
        .expect("insert history v2");
    assert!(second.changed);
    assert_eq!(second.version, 2);

    let history = store
        .list_sir_history("sym-history")
        .expect("list ordered history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].version, 1);
    assert_eq!(history[0].sir_hash, "hash-1");
    assert_eq!(
        history[0].commit_hash.as_deref(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(history[1].version, 2);
    assert_eq!(history[1].sir_hash, "hash-2");
    assert_eq!(
        history[1].commit_hash.as_deref(),
        Some("2222222222222222222222222222222222222222")
    );

    drop(store);

    let reopened = SqliteStore::open(workspace).expect("reopen store");
    let reopened_history = reopened
        .list_sir_history("sym-history")
        .expect("list history after reopen");
    assert_eq!(reopened_history, history);
}

#[test]
fn resolve_sir_history_pair_supports_versions_and_timestamps() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    store
        .record_sir_version_if_changed(
            "sym-history",
            "hash-1",
            "mock",
            "mock",
            "{\"intent\":\"v1\"}",
            1_700_300_100,
            Some("1111111111111111111111111111111111111111"),
        )
        .expect("insert history v1");
    store
        .record_sir_version_if_changed(
            "sym-history",
            "hash-2",
            "mock",
            "mock",
            "{\"intent\":\"v2\"}",
            1_700_300_200,
            Some("2222222222222222222222222222222222222222"),
        )
        .expect("insert history v2");

    let by_version = store
        .resolve_sir_history_pair(
            "sym-history",
            SirHistorySelector::Version(1),
            SirHistorySelector::Version(2),
        )
        .expect("resolve by version")
        .expect("pair should exist");
    assert_eq!(by_version.from.version, 1);
    assert_eq!(by_version.to.version, 2);
    assert_eq!(
        by_version.from.commit_hash.as_deref(),
        Some("1111111111111111111111111111111111111111")
    );
    assert_eq!(
        by_version.to.commit_hash.as_deref(),
        Some("2222222222222222222222222222222222222222")
    );

    let by_timestamp = store
        .resolve_sir_history_pair(
            "sym-history",
            SirHistorySelector::CreatedAt(1_700_300_150),
            SirHistorySelector::CreatedAt(1_700_300_250),
        )
        .expect("resolve by timestamp")
        .expect("timestamp pair should exist");
    assert_eq!(by_timestamp.from.version, 1);
    assert_eq!(by_timestamp.to.version, 2);

    let unresolved = store
        .resolve_sir_history_pair(
            "sym-history",
            SirHistorySelector::Version(2),
            SirHistorySelector::Version(1),
        )
        .expect("resolve reversed pair");
    assert!(unresolved.is_none());
}

#[test]
fn record_sir_version_unchanged_hash_advances_timestamp() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open");
    let symbol_id = "test::unchanged_ts";
    let sir_hash = "abc123";
    let sir_json = r#"{"intent":"test"}"#;

    let first = store
        .record_sir_version_if_changed(symbol_id, sir_hash, "test", "test", sir_json, 1000, None)
        .expect("first write");
    assert!(first.changed);
    assert_eq!(first.updated_at, 1000);

    let second = store
        .record_sir_version_if_changed(symbol_id, sir_hash, "test", "test", sir_json, 2000, None)
        .expect("second write");
    assert!(!second.changed);
    assert_eq!(second.updated_at, 2000);
}

#[test]
fn latest_sir_history_pair_handles_empty_single_and_multiple_history() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");

    let empty = store
        .latest_sir_history_pair("missing")
        .expect("query empty history");
    assert!(empty.is_none());

    store
        .record_sir_version_if_changed(
            "sym-latest",
            "hash-1",
            "mock",
            "mock",
            "{\"intent\":\"v1\"}",
            1_700_310_100,
            None,
        )
        .expect("insert single version");
    let single = store
        .latest_sir_history_pair("sym-latest")
        .expect("query single history")
        .expect("single pair");
    assert_eq!(single.from.version, 1);
    assert_eq!(single.to.version, 1);

    store
        .record_sir_version_if_changed(
            "sym-latest",
            "hash-2",
            "mock",
            "mock",
            "{\"intent\":\"v2\"}",
            1_700_310_200,
            None,
        )
        .expect("insert second version");
    let multiple = store
        .latest_sir_history_pair("sym-latest")
        .expect("query multiple history")
        .expect("multiple pair");
    assert_eq!(multiple.from.version, 1);
    assert_eq!(multiple.to.version, 2);
}

#[test]
fn mirror_write_can_be_disabled_via_config() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[inference]
provider = "auto"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = false
"#,
    )
    .expect("write config");

    let store = SqliteStore::open(workspace).expect("open store");
    assert!(!store.mirror_sir_files_enabled());

    store
        .write_sir_blob("sym-1", "{\"intent\":\"sqlite-only\"}")
        .expect("write sqlite-only");

    let mirror_path = workspace.join(".aether/sir/sym-1.json");
    assert!(!mirror_path.exists());

    let loaded = store.read_sir_blob("sym-1").expect("read sqlite-only");
    assert_eq!(loaded.as_deref(), Some("{\"intent\":\"sqlite-only\"}"));
}

#[test]
fn open_store_backfills_sir_history_from_existing_sir_rows() {
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
    .expect("create legacy schema with sir_json");

    conn.execute(
            "INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at, sir_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "legacy-history",
                "legacy-hash",
                3i64,
                "mock",
                "mock",
                1_700_222_333i64,
                "{\"intent\":\"legacy\"}"
            ],
        )
        .expect("insert legacy sir row with json");
    drop(conn);

    let store = SqliteStore::open(workspace).expect("open migrated store");
    let history = store
        .list_sir_history("legacy-history")
        .expect("load migrated history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].version, 3);
    assert_eq!(history[0].sir_hash, "legacy-hash");
    assert_eq!(history[0].sir_json, "{\"intent\":\"legacy\"}");
    assert_eq!(history[0].created_at, 1_700_222_333);
    assert_eq!(history[0].commit_hash, None);
}

#[test]
fn count_symbols_with_sir_reports_total_and_with_sir() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_symbol(symbol_record())
        .expect("upsert first symbol");
    store
        .upsert_symbol(symbol_record_ts())
        .expect("upsert second symbol");
    store
            .write_sir_blob(
                "sym-1",
                r#"{"intent":"ok","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.9}"#,
            )
            .expect("write sir blob");

    let (total, with_sir) = store
        .count_symbols_with_sir()
        .expect("count symbols with sir");
    assert_eq!(total, 2);
    assert_eq!(with_sir, 1);
}

#[test]
fn resolve_sir_baseline_by_selector_supports_versions_timestamps_and_commits() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .record_sir_version_if_changed(
            "sym-baseline",
            "hash-1",
            "mock",
            "mock",
            "{\"intent\":\"v1\"}",
            1_700_000_000_000,
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .expect("insert baseline v1");
    store
        .record_sir_version_if_changed(
            "sym-baseline",
            "hash-2",
            "mock",
            "mock",
            "{\"intent\":\"v2\"}",
            1_700_000_000_100,
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        )
        .expect("insert baseline v2");

    let by_version = store
        .resolve_sir_baseline_by_selector("sym-baseline", SirHistoryBaselineSelector::Version(1))
        .expect("resolve by version")
        .expect("version baseline exists");
    assert_eq!(by_version.version, 1);

    let by_time = store
        .resolve_sir_baseline_by_selector(
            "sym-baseline",
            SirHistoryBaselineSelector::CreatedAt(1_700_000_000_050),
        )
        .expect("resolve by timestamp")
        .expect("timestamp baseline exists");
    assert_eq!(by_time.version, 1);

    let by_commit = store
        .resolve_sir_baseline_by_selector(
            "sym-baseline",
            SirHistoryBaselineSelector::CommitHash("bbbb".to_owned()),
        )
        .expect("resolve by commit hash")
        .expect("commit baseline exists");
    assert_eq!(by_commit.version, 2);
}

#[test]
fn list_symbol_ids_without_sir_returns_missing_only() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_symbol(symbol_record())
        .expect("upsert first symbol");
    store
        .upsert_symbol(symbol_record_ts())
        .expect("upsert second symbol");
    store
            .write_sir_blob(
                "sym-1",
                r#"{"intent":"ok","inputs":[],"outputs":[],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.9}"#,
            )
            .expect("write sir blob");

    let missing = store
        .list_symbol_ids_without_sir()
        .expect("list symbol ids without sir");
    assert_eq!(missing, vec!["sym-2".to_owned()]);
}

#[test]
fn sir_request_queue_round_trip_works() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store.enqueue_sir_request("sym-a").expect("enqueue sym-a");
    store.enqueue_sir_request("sym-b").expect("enqueue sym-b");

    let listed = store
        .list_sir_request_symbol_ids(10)
        .expect("list sir requests");
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&"sym-a".to_owned()));
    assert!(listed.contains(&"sym-b".to_owned()));

    let consumed = store
        .consume_sir_requests(10)
        .expect("consume sir requests");
    assert_eq!(consumed.len(), 2);
    assert!(
        store
            .list_sir_request_symbol_ids(10)
            .expect("list after consume")
            .is_empty()
    );

    store.enqueue_sir_request("sym-c").expect("enqueue sym-c");
    store.clear_sir_request("sym-c").expect("clear sym-c");
    assert!(
        store
            .list_sir_request_symbol_ids(10)
            .expect("list after clear")
            .is_empty()
    );
}
