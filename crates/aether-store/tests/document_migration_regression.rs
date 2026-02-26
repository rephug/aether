use aether_store::{SirMetaRecord, SqliteStore, Store, SymbolRecord};
use tempfile::tempdir;

#[test]
fn existing_symbol_and_sir_operations_still_work_after_migration_v2() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let schema = store.get_schema_version().expect("schema version");
    assert_eq!(schema.version, 2);

    store
        .upsert_symbol(SymbolRecord {
            id: "sym-1".to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "demo::run".to_owned(),
            signature_fingerprint: "sig-1".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .expect("upsert symbol");

    let meta = SirMetaRecord {
        id: "sym-1".to_owned(),
        sir_hash: "hash-1".to_owned(),
        sir_version: 1,
        provider: "mock".to_owned(),
        model: "mock".to_owned(),
        updated_at: 1_700_000_100,
        sir_status: "fresh".to_owned(),
        last_error: None,
        last_attempt_at: 1_700_000_100,
    };
    store
        .upsert_sir_meta(meta.clone())
        .expect("upsert sir meta");
    let loaded = store.get_sir_meta("sym-1").expect("get sir meta");
    assert_eq!(loaded, Some(meta));
}
