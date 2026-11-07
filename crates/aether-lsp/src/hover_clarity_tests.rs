//! Real parser/store regressions for wording and saved-source freshness.
use super::*;
use aether_store::{SirMetaRecord, SirStateStore};

const SOURCE: &str = "fn increment(x: i32) -> i32 { x + 1 }\n";
const EXPLANATION: &str = r#"{"intent":"Adds one to the supplied number.","inputs":["x: the starting number"],"outputs":["The starting number plus one"],"side_effects":[],"dependencies":[],"error_modes":[],"confidence":0.9}"#;

#[test]
fn canonical_paths_and_editor_paths_share_catalog_identities() {
    let temp = tempfile::tempdir().expect("workspace");
    let file = temp.path().join("lib.rs");
    std::fs::write(&file, SOURCE).expect("source");
    let canonical_root = temp.path().canonicalize().expect("canonical root");
    let canonical_file = file.canonicalize().expect("canonical file");
    assert_eq!(
        workspace_relative_display_path(&canonical_root, &file),
        "lib.rs"
    );
    assert_eq!(
        workspace_relative_display_path(temp.path(), &canonical_file),
        "lib.rs"
    );
}

fn seed(store: &SqliteStore, source: &str, source_known: bool) -> String {
    let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
    let symbol = extractor
        .extract_from_source(Language::Rust, "lib.rs", source)
        .expect("parse")
        .remove(0);
    let id = symbol.id.clone();
    let explanation = if source.contains("x + 2") {
        EXPLANATION.replace("one", "two")
    } else {
        EXPLANATION.to_owned()
    };
    store
        .persist_sir_state_atomically_with_source(
            SirMetaRecord {
                id: id.clone(),
                sir_hash: aether_core::content_hash(&explanation),
                sir_version: 1,
                provider: "test-fixture".into(),
                model: "test-fixture".into(),
                generation_pass: "single".into(),
                reasoning_trace: None,
                prompt_hash: None,
                staleness_score: None,
                updated_at: 1_700_000_000,
                sir_status: "fresh".into(),
                last_error: None,
                last_attempt_at: 1_700_000_000,
            },
            &explanation,
            None,
            None,
            source_known.then_some(symbol.content_hash.as_str()),
        )
        .expect("persist explanation with source");
    id
}

#[test]
fn saved_body_edit_warns_before_index_status_catches_up() {
    let temp = tempfile::tempdir().expect("workspace");
    let file = temp.path().join("lib.rs");
    std::fs::write(&file, SOURCE).expect("source");
    let store = SqliteStore::open(temp.path()).expect("store");
    let id = seed(&store, SOURCE, true);
    let before = resolve_hover_markdown_for_path(temp.path(), &store, &file, Position::new(0, 5))
        .expect("hover")
        .expect("explanation");
    assert!(!before.contains("may be out of date"));
    assert!(!before.contains("Could not check whether"));

    // Same signature and symbol id, different saved behavior. No indexer runs.
    std::fs::write(&file, SOURCE.replace("x + 1", "x + 2")).expect("save edit");
    let after = resolve_hover_markdown_for_path(temp.path(), &store, &file, Position::new(0, 5))
        .expect("hover")
        .expect("explanation");
    assert_eq!(
        store
            .get_sir_meta(&id)
            .expect("metadata")
            .expect("row")
            .sir_status,
        "fresh"
    );
    assert!(after.contains("This explanation may be out of date"));
    assert!(after.contains("saved code has changed"));
    println!("STALE_EXAMPLE_BEGIN\n{after}\nSTALE_EXAMPLE_END");
    assert!(after.contains("Adds one")); // Preserved, clearly marked as older.
    assert!(
        store
            .list_sir_request_symbol_ids(10)
            .expect("requests")
            .is_empty()
    );

    seed(&store, &SOURCE.replace("x + 1", "x + 2"), true);
    let refreshed =
        resolve_hover_markdown_for_path(temp.path(), &store, &file, Position::new(0, 5))
            .expect("hover")
            .expect("explanation");
    assert!(!refreshed.contains("may be out of date"));
    assert!(refreshed.contains("Adds two"));
}

#[test]
fn failed_queue_write_does_not_claim_an_explanation_was_requested() {
    let temp = tempfile::tempdir().expect("workspace");
    drop(SqliteStore::open(temp.path()).expect("initialize"));
    let store = SqliteStore::open_readonly(temp.path()).expect("readonly store");
    let mut extractor = aether_parse::SymbolExtractor::new().expect("extractor");
    let symbol = extractor
        .extract_from_source(Language::Rust, "lib.rs", SOURCE)
        .expect("parse")
        .remove(0);
    let hover = render_pending_hover_markdown(&store, &symbol.id, &symbol, None);
    assert!(hover.contains("could not be requested"));
    assert!(hover.contains("**Request details**"));
    assert!(!hover.contains("Waiting for the result"));
    assert!(
        store
            .list_sir_request_symbol_ids(10)
            .expect("queue")
            .is_empty()
    );
}

#[test]
fn missing_source_fingerprint_is_disclosed_as_unknown() {
    let temp = tempfile::tempdir().expect("workspace");
    let file = temp.path().join("lib.rs");
    std::fs::write(&file, SOURCE).expect("source");
    let store = SqliteStore::open(temp.path()).expect("store");
    let id = seed(&store, SOURCE, false);
    for recorded_source in [None, Some(""), Some("legacy-source")] {
        let meta = store.get_sir_meta(&id).expect("metadata").expect("row");
        store
            .persist_sir_state_atomically_with_source(
                meta,
                EXPLANATION,
                None,
                None,
                recorded_source,
            )
            .expect("legacy source metadata");
        let hover =
            resolve_hover_markdown_for_path(temp.path(), &store, &file, Position::new(0, 5))
                .expect("hover")
                .expect("explanation");
        assert!(hover.contains("Could not check whether"));
        assert!(!hover.contains("saved code has changed"));
    }
}

#[test]
fn absent_explanation_is_queued_without_claiming_a_worker_is_running() {
    let temp = tempfile::tempdir().expect("workspace");
    let file = temp.path().join("lib.rs");
    std::fs::write(&file, SOURCE).expect("source");
    let store = SqliteStore::open(temp.path()).expect("store");
    let hover = resolve_hover_markdown_for_path(temp.path(), &store, &file, Position::new(0, 5))
        .expect("hover")
        .expect("explanation");
    assert!(hover.contains("Waiting for the result"));
    assert!(!hover.contains("in progress"));
    assert!(!hover.contains("SIR"));
    assert_eq!(
        store.list_sir_request_symbol_ids(10).expect("queue").len(),
        1
    );
}

#[test]
fn plain_summary_and_uncertainty_precede_technical_details() {
    let temp = tempfile::tempdir().expect("workspace");
    let file = temp.path().join("lib.rs");
    std::fs::write(&file, SOURCE).expect("source");
    let store = SqliteStore::open(temp.path()).expect("store");
    seed(&store, SOURCE, true);
    let hover = resolve_hover_markdown_for_path(temp.path(), &store, &file, Position::new(0, 5))
        .expect("hover")
        .expect("explanation");
    assert!(hover.starts_with("### increment\n\n**What this does**\nAdds one"));
    println!("HOVER_EXAMPLE_BEGIN\n{hover}\nHOVER_EXAMPLE_END");
    assert!(
        hover.find("**Result**").expect("result")
            < hover.find("**Technical details**").expect("details")
    );
    assert!(hover.contains("No cautions were recorded. This does not confirm"));
    assert!(!hover.contains("(none)"));
    assert!(hover.contains("not a correctness check"));
}
