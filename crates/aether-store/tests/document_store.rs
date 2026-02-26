use aether_document::{GenericRecord, GenericUnit};
use aether_store::document_store::DomainStats;
use aether_store::SqliteStore;
use serde_json::json;
use tempfile::tempdir;

#[test]
fn document_store_crud_and_domain_stats_work() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut unit = GenericUnit::new(
        "Intro",
        "AETHER is a local-first code intelligence system.",
        "paragraph",
        "docs/intro.md",
        (0, 48),
        None,
        "docs",
    );
    unit.metadata_json = json!({"section":"intro","lang":"en"});
    store
        .insert_document_unit(&unit)
        .expect("insert document unit");

    let fetched = store
        .get_document_unit(unit.unit_id.as_str())
        .expect("get by id")
        .expect("unit exists");
    assert_eq!(fetched.unit_id, unit.unit_id);
    assert_eq!(fetched.byte_range, (0, 48));
    assert_eq!(fetched.metadata_json, json!({"section":"intro","lang":"en"}));

    let by_domain = store.get_units_by_domain("docs").expect("units by domain");
    assert_eq!(by_domain.len(), 1);
    assert_eq!(by_domain[0].unit_id, unit.unit_id);

    let by_source = store
        .get_units_by_source("docs/intro.md")
        .expect("units by source");
    assert_eq!(by_source.len(), 1);
    assert_eq!(by_source[0].unit_id, unit.unit_id);

    let record = GenericRecord::new(
        unit.unit_id.clone(),
        "docs",
        "entity",
        "v1",
        json!({"title":"AETHER","tags":["system","local"]}),
        "AETHER local-first system",
    )
    .expect("record");
    store
        .insert_semantic_record(&record)
        .expect("insert semantic record");

    let record_by_unit = store
        .get_record_by_unit(unit.unit_id.as_str())
        .expect("record by unit")
        .expect("record exists");
    assert_eq!(record_by_unit.record_id, record.record_id);
    assert_eq!(record_by_unit.domain, "docs");

    let records_by_domain = store
        .get_records_by_domain("docs", 10)
        .expect("records by domain");
    assert_eq!(records_by_domain.len(), 1);
    assert_eq!(records_by_domain[0].record_id, record.record_id);

    let lexical_embedding = store
        .search_records_lexical("docs", "local-first", 10)
        .expect("lexical search embedding_text");
    assert_eq!(lexical_embedding.len(), 1);
    assert_eq!(lexical_embedding[0].record_id, record.record_id);

    let lexical_json = store
        .search_records_lexical("docs", "\"tags\":[\"system\"", 10)
        .expect("lexical search record_json");
    assert_eq!(lexical_json.len(), 1);
    assert_eq!(lexical_json[0].record_id, record.record_id);

    let stats = store.domain_stats("docs").expect("domain stats");
    assert_eq!(
        stats,
        DomainStats {
            unit_count: 1,
            record_count: 1,
            source_count: 1,
            last_updated: stats.last_updated,
        }
    );
    assert!(stats.last_updated.unwrap_or_default() > 0);

    let deleted = store
        .delete_units_by_source("docs/intro.md")
        .expect("delete by source");
    assert_eq!(deleted, 1);

    assert!(
        store
            .get_document_unit(unit.unit_id.as_str())
            .expect("get deleted unit")
            .is_none()
    );
    assert!(
        store
            .get_record_by_unit(unit.unit_id.as_str())
            .expect("record deleted with unit")
            .is_none()
    );
}
