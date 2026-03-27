use super::*;

#[test]
fn store_creates_layout_and_persists_data_without_duplicates() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();

    let store = SqliteStore::open(workspace).expect("open store");
    assert!(store.aether_dir().exists());
    assert!(store.sir_dir().exists());
    assert!(store.mirror_sir_files_enabled());
    assert!(store.aether_dir().join("meta.sqlite").exists());

    let mut record = symbol_record();
    store
        .upsert_symbol(record.clone())
        .expect("upsert symbol first time");

    record.last_seen_at = 1_700_000_200;
    record.signature_fingerprint = "sig-b".to_owned();
    store
        .upsert_symbol(record.clone())
        .expect("upsert symbol second time");

    let list = store
        .list_symbols_for_file("src/lib.rs")
        .expect("list symbols after upsert");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0], record);

    store
        .write_sir_blob("sym-1", "{\"intent\":\"demo\"}")
        .expect("write blob");
    let blob = store.read_sir_blob("sym-1").expect("read blob");
    assert_eq!(blob.as_deref(), Some("{\"intent\":\"demo\"}"));

    let sir_meta = sir_meta_record();
    store
        .upsert_sir_meta(sir_meta.clone())
        .expect("upsert sir meta");
    let loaded_meta = store.get_sir_meta("sym-1").expect("get sir meta");
    assert_eq!(loaded_meta, Some(sir_meta));

    drop(store);

    let reopened = SqliteStore::open(workspace).expect("reopen store");
    let reopened_list = reopened
        .list_symbols_for_file("src/lib.rs")
        .expect("list symbols after reopen");
    assert_eq!(reopened_list.len(), 1);
    assert_eq!(reopened_list[0], record);

    let reopened_blob = reopened
        .read_sir_blob("sym-1")
        .expect("read blob after reopen");
    assert_eq!(reopened_blob.as_deref(), Some("{\"intent\":\"demo\"}"));

    let reopened_meta = reopened.get_sir_meta("sym-1").expect("meta after reopen");
    assert_eq!(reopened_meta, Some(sir_meta_record()));
}

#[test]
fn embedding_records_persist_and_search_semantic_ranks_expected_match() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_symbol(symbol_record())
        .expect("upsert first symbol");
    let mut second = symbol_record_ts();
    second.id = "sym-2".to_owned();
    second.qualified_name = "demo::network_retry".to_owned();
    store.upsert_symbol(second).expect("upsert second symbol");

    store
        .upsert_symbol_embedding(embedding_record("sym-1", "hash-a", vec![1.0, 0.0]))
        .expect("upsert first embedding");
    store
        .upsert_symbol_embedding(embedding_record("sym-2", "hash-b", vec![0.0, 1.0]))
        .expect("upsert second embedding");

    let meta = store
        .get_symbol_embedding_meta("sym-1")
        .expect("read embedding meta")
        .expect("embedding meta exists");
    assert_eq!(meta.sir_hash, "hash-a");
    assert_eq!(meta.embedding_dim, 2);

    let semantic = store
        .search_symbols_semantic(&[0.0, 1.0], "mock", "mock-64d", 5)
        .expect("semantic search");
    assert!(!semantic.is_empty());
    assert_eq!(semantic[0].symbol_id, "sym-2");
    assert!(semantic[0].semantic_score > semantic[1].semantic_score);
}

#[test]
fn edge_records_can_be_upserted_queried_and_deleted_by_file() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_edges(&[
            calls_edge("sym-alpha", "beta", "src/lib.rs"),
            depends_edge("file::src/app.ts", "./dep", "src/app.ts"),
        ])
        .expect("upsert edges");

    let callers = store.get_callers("beta").expect("get callers");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0], calls_edge("sym-alpha", "beta", "src/lib.rs"));

    let deps = store
        .get_dependencies("file::src/app.ts")
        .expect("get dependencies");
    assert_eq!(deps.len(), 1);
    assert_eq!(
        deps[0],
        depends_edge("file::src/app.ts", "./dep", "src/app.ts")
    );

    store
        .delete_edges_for_file("src/lib.rs")
        .expect("delete edges for file");
    let callers_after_delete = store.get_callers("beta").expect("get callers after delete");
    assert!(callers_after_delete.is_empty());

    let deps_after_delete = store
        .get_dependencies("file::src/app.ts")
        .expect("get dependencies after delete");
    assert_eq!(deps_after_delete.len(), 1);
}

#[tokio::test]
async fn sync_graph_for_file_skips_unresolved_calls() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let graph = SurrealGraphStore::open(temp.path())
        .await
        .expect("open surreal graph store");

    let alpha = SymbolRecord {
        id: "sym-alpha".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "alpha".to_owned(),
        signature_fingerprint: "sig-alpha".to_owned(),
        last_seen_at: 1_700_000_000,
    };
    store.upsert_symbol(alpha.clone()).expect("upsert symbol");
    store
        .upsert_edges(&[calls_edge(&alpha.id, "missing::target", "src/lib.rs")])
        .expect("upsert unresolved edge");

    let stats = store
        .sync_graph_for_file(&graph, "src/lib.rs")
        .await
        .expect("sync graph for file");
    assert_eq!(stats.resolved_edges, 0);
    assert_eq!(stats.unresolved_edges, 1);

    let deps = graph
        .get_dependencies(&alpha.id)
        .await
        .expect("query dependencies");
    assert!(deps.is_empty());
}

#[tokio::test]
async fn sync_graph_for_file_syncs_type_ref_and_implements_edges() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let graph = SurrealGraphStore::open(temp.path())
        .await
        .expect("open surreal graph store");

    let alpha = SymbolRecord {
        id: "sym-alpha".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: "alpha".to_owned(),
        signature_fingerprint: "sig-alpha".to_owned(),
        last_seen_at: 1_700_000_000,
    };
    let target = SymbolRecord {
        id: "sym-target".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "struct".to_owned(),
        qualified_name: "Target".to_owned(),
        signature_fingerprint: "sig-target".to_owned(),
        last_seen_at: 1_700_000_000,
    };
    let store_trait = SymbolRecord {
        id: "sym-store".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "trait".to_owned(),
        qualified_name: "Store".to_owned(),
        signature_fingerprint: "sig-store".to_owned(),
        last_seen_at: 1_700_000_000,
    };
    let impl_type = SymbolRecord {
        id: "sym-impl".to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "struct".to_owned(),
        qualified_name: "SqliteStore".to_owned(),
        signature_fingerprint: "sig-impl".to_owned(),
        last_seen_at: 1_700_000_000,
    };

    for symbol in [&alpha, &target, &store_trait, &impl_type] {
        store.upsert_symbol(symbol.clone()).expect("upsert symbol");
    }
    store
        .upsert_edges(&[
            type_ref_edge(&alpha.id, "Target", "src/lib.rs"),
            implements_edge(&impl_type.id, "Store", "src/lib.rs"),
        ])
        .expect("upsert structural edges");

    let stats = store
        .sync_graph_for_file(&graph, "src/lib.rs")
        .await
        .expect("sync graph for file");
    assert_eq!(stats.resolved_edges, 2);
    assert_eq!(stats.unresolved_edges, 0);

    let edges = graph
        .list_dependency_edges()
        .await
        .expect("list dependency edges");
    assert!(edges.iter().any(|edge| {
        edge.source_symbol_id == alpha.id
            && edge.target_symbol_id == target.id
            && edge.edge_kind == "type_ref"
    }));
    assert!(edges.iter().any(|edge| {
        edge.source_symbol_id == impl_type.id
            && edge.target_symbol_id == store_trait.id
            && edge.edge_kind == "implements"
    }));
}

#[test]
fn mark_removed_deletes_symbol_row() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_symbol(symbol_record())
        .expect("upsert symbol before delete");
    store
        .write_sir_blob("sym-1", "{\"intent\":\"to-remove\"}")
        .expect("write sir before delete");
    store
        .upsert_symbol_embedding(embedding_record("sym-1", "hash-remove", vec![1.0, 0.0]))
        .expect("write embedding before delete");
    store
        .record_sir_version_if_changed(
            "sym-1",
            "hash-remove",
            "mock",
            "mock",
            "{\"intent\":\"to-remove\"}",
            1_700_111_000,
            None,
        )
        .expect("insert history before delete");
    store.mark_removed("sym-1").expect("mark removed");

    let list = store
        .list_symbols_for_file("src/lib.rs")
        .expect("list after delete");
    assert!(list.is_empty());

    let sir = store.read_sir_blob("sym-1").expect("sir after delete");
    assert!(sir.is_none());

    let embedding_meta = store
        .get_symbol_embedding_meta("sym-1")
        .expect("embedding metadata after delete");
    assert!(embedding_meta.is_none());

    let history = store
        .list_sir_history("sym-1")
        .expect("history after delete");
    assert!(history.is_empty());
}

#[test]
fn mark_removed_cleans_all_live_symbol_tables_and_preserves_history() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let symbol_id = "sym-cleanup";

    seed_live_symbol_cleanup_state(&store, symbol_id);
    let mirrored_sir_path = store.sir_dir().join(format!("{symbol_id}.json"));
    assert!(mirrored_sir_path.exists());

    store.mark_removed(symbol_id).expect("mark removed");

    assert_live_symbol_cleanup_empty(&store, symbol_id);
    assert_preserved_symbol_history_retained(&store, symbol_id);
    assert_eq!(
        store
            .read_sir_blob(symbol_id)
            .expect("read sir after cleanup"),
        None
    );
    assert!(!mirrored_sir_path.exists());
}

#[test]
fn mark_removed_rolls_back_when_delete_fails() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    let symbol_id = "sym-rollback";

    seed_live_symbol_cleanup_state(&store, symbol_id);
    {
        let conn = store.conn.lock().expect("lock store connection");
        conn.execute_batch(
            format!(
                r#"
                CREATE TRIGGER fail_symbol_delete_for_test
                BEFORE DELETE ON symbols
                WHEN OLD.id = '{symbol_id}'
                BEGIN
                    SELECT RAISE(FAIL, 'symbol delete blocked for test');
                END;
                "#
            )
            .as_str(),
        )
        .expect("install rollback trigger");
    }

    let err = store
        .mark_removed(symbol_id)
        .expect_err("mark removed should roll back on trigger failure");
    assert!(err.to_string().contains("symbol delete blocked for test"));
    assert!(
        count_symbol_rows(
            &store,
            "SELECT COUNT(*) FROM sir_embeddings WHERE symbol_id = ?1",
            symbol_id,
        ) > 0
    );
    assert!(
        count_symbol_rows(
            &store,
            "SELECT COUNT(*) FROM sir_history WHERE symbol_id = ?1",
            symbol_id,
        ) > 0
    );
    assert!(
        count_symbol_rows(
            &store,
            "SELECT COUNT(*) FROM write_intents WHERE symbol_id = ?1",
            symbol_id,
        ) > 0
    );
    assert!(count_symbol_rows(&store, "SELECT COUNT(*) FROM sir WHERE id = ?1", symbol_id,) > 0);
    assert!(
        count_symbol_rows(
            &store,
            "SELECT COUNT(*) FROM symbols WHERE id = ?1",
            symbol_id,
        ) > 0
    );
    assert_preserved_symbol_history_retained(&store, symbol_id);
    assert!(store.sir_dir().join(format!("{symbol_id}.json")).exists());
}

#[test]
fn search_symbols_matches_by_name_path_language_and_kind() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    store
        .upsert_symbol(symbol_record())
        .expect("upsert rust symbol");
    store
        .upsert_symbol(symbol_record_ts())
        .expect("upsert ts symbol");

    let by_name = store
        .search_symbols("demo::run", 20)
        .expect("search by name");
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0].symbol_id, "sym-1");

    let by_path = store
        .search_symbols("src/app.ts", 20)
        .expect("search by path");
    assert_eq!(by_path.len(), 1);
    assert_eq!(by_path[0].symbol_id, "sym-2");

    let by_language = store
        .search_symbols("RUST", 20)
        .expect("search by language");
    assert_eq!(by_language.len(), 1);
    assert_eq!(by_language[0].symbol_id, "sym-1");

    let by_kind = store
        .search_symbols("function", 20)
        .expect("search by kind");
    assert_eq!(by_kind.len(), 2);
}

#[test]
fn search_symbols_respects_empty_query_and_limit() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut first = symbol_record();
    first.qualified_name = "alpha::run".to_owned();
    first.id = "sym-a".to_owned();
    store.upsert_symbol(first).expect("upsert first symbol");

    let mut second = symbol_record();
    second.qualified_name = "beta::run".to_owned();
    second.id = "sym-b".to_owned();
    store.upsert_symbol(second).expect("upsert second symbol");

    let empty = store.search_symbols("   ", 20).expect("search empty");
    assert!(empty.is_empty());

    let limited = store.search_symbols("::run", 1).expect("search with limit");
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].qualified_name, "alpha::run");
}

#[test]
fn increment_symbol_access_updates_count_and_timestamp() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    store.upsert_symbol(symbol_record()).expect("upsert symbol");

    store
        .increment_symbol_access(&["sym-1".to_owned()], 1_700_100_000)
        .expect("increment symbol access");

    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT access_count, last_accessed_at FROM symbols WHERE id = ?1")
        .expect("prepare symbol access lookup");
    let access = stmt
        .query_row(params!["sym-1"], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .expect("query access fields");

    assert_eq!(access.0, 1);
    assert_eq!(access.1, Some(1_700_100_000));
}

#[test]
fn increment_symbol_access_debounced_skips_duplicate_within_window() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    store.upsert_symbol(symbol_record()).expect("upsert symbol");

    store
        .increment_symbol_access_debounced(&["sym-1".to_owned()], 1_700_100_100)
        .expect("first debounced increment");
    store
        .increment_symbol_access_debounced(&["sym-1".to_owned()], 1_700_100_101)
        .expect("second debounced increment");

    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT access_count, last_accessed_at FROM symbols WHERE id = ?1")
        .expect("prepare symbol access lookup");
    let access = stmt
        .query_row(params!["sym-1"], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .expect("query access fields");

    assert_eq!(access.0, 1);
    assert_eq!(access.1, Some(1_700_100_100));
}

#[test]
fn increment_symbol_access_debounced_handles_future_timestamps_without_panicking() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    store.upsert_symbol(symbol_record()).expect("upsert symbol");

    {
        let mut tracker = store
            .symbol_access_debounce
            .lock()
            .expect("lock debounce tracker");
        tracker.insert(
            "sym-1".to_owned(),
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        );
    }

    store
        .increment_symbol_access_debounced(&["sym-1".to_owned()], 1_700_100_200)
        .expect("debounced increment with future timestamp");

    let conn = store.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT access_count, last_accessed_at FROM symbols WHERE id = ?1")
        .expect("prepare symbol access lookup");
    let access = stmt
        .query_row(params!["sym-1"], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .expect("query access fields");

    assert_eq!(access.0, 0);
    assert_eq!(access.1, None);
}

#[test]
fn open_readonly_store_rejects_insert_operations() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");
    drop(store);

    let readonly = SqliteStore::open_readonly(workspace).expect("open readonly store");
    let err = readonly
        .upsert_symbol(symbol_record())
        .expect_err("readonly insert should fail");

    match err {
        StoreError::Sqlite(inner) => {
            let message = inner.to_string().to_ascii_lowercase();
            assert!(
                message.contains("readonly")
                    || message.contains("read-only")
                    || message.contains("attempt to write"),
                "unexpected sqlite error: {message}"
            );
        }
        other => panic!("expected sqlite readonly error, got {other}"),
    }
}

#[test]
fn open_readonly_store_access_increments_are_noop() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    let store = SqliteStore::open(workspace).expect("open store");
    store.upsert_symbol(symbol_record()).expect("upsert symbol");
    store
        .upsert_project_note(project_note_record(
            "note-1",
            "Pinned architecture decision",
            &["memory"],
            1_700_200_000,
        ))
        .expect("upsert project note");
    drop(store);

    let readonly = SqliteStore::open_readonly(workspace).expect("open readonly store");
    readonly
        .increment_symbol_access(&["sym-1".to_owned()], 1_700_200_100)
        .expect("readonly symbol increment should noop");
    readonly
        .increment_project_note_access(&["note-1".to_owned()], 1_700_200_100)
        .expect("readonly note increment should noop");

    let conn = readonly.conn.lock().unwrap();
    let symbol_access = conn
        .query_row(
            "SELECT access_count, last_accessed_at FROM symbols WHERE id = ?1",
            params!["sym-1"],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .expect("query symbol access");
    assert_eq!(symbol_access.0, 0);
    assert_eq!(symbol_access.1, None);

    let note_access = conn
        .query_row(
            "SELECT access_count, last_accessed_at FROM project_notes WHERE note_id = ?1",
            params!["note-1"],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .expect("query note access");
    assert_eq!(note_access.0, 0);
    assert_eq!(note_access.1, None);
}

#[test]
fn get_symbol_metadata_reports_visibility_and_line_count() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    fs::create_dir_all(workspace.join("src")).expect("create src dir");
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn alpha() {}\nfn beta() {}\n",
    )
    .expect("write source");

    let store = SqliteStore::open(workspace).expect("open store");
    store
        .upsert_symbol(SymbolRecord {
            id: "sym-alpha".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "alpha".to_owned(),
            signature_fingerprint: "sig-alpha".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert symbol");

    let metadata = store
        .get_symbol_metadata("sym-alpha")
        .expect("get metadata")
        .expect("metadata exists");
    assert_eq!(metadata.file_path, "src/lib.rs");
    assert_eq!(metadata.kind, "function");
    assert!(metadata.is_public);
    assert_eq!(metadata.line_count, 2);
}
