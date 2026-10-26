use std::fs;
use std::path::Path;

use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore};
use aetherd::indexer::{IndexerConfig, run_initial_index_once};
use aetherd::preflight::{PreflightRequest, assemble_preflight};
use tempfile::tempdir;

const CONFIG: &str = "[inference]\nprovider = \"mock\"\n[storage]\ngraph_backend = \"sqlite\"\n[embeddings]\nenabled = false\nvector_backend = \"sqlite\"\n";
const CLEAN: &str = "pub struct Secret;\nimpl std::fmt::Debug for Secret { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(\"secret\") } }\n";
const DISPLAY: &str = "impl std::fmt::Display for Secret { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(\"secret\") } }\n";

fn config(workspace: &Path) -> IndexerConfig {
    IndexerConfig {
        workspace: workspace.to_path_buf(),
        debounce_ms: 300,
        print_events: false,
        print_sir: false,
        embeddings_only: false,
        force: false,
        full: false,
        deep: false,
        turbo_concurrency: None,
        dry_run: false,
        sir_concurrency: 1,
        lifecycle_logs: false,
        inference_provider: None,
        inference_model: None,
        inference_endpoint: None,
        inference_api_key_env: None,
        pause_flag: None,
    }
}

fn seed(workspace: &Path, colliding: bool) {
    fs::create_dir_all(workspace.join(".aether")).expect("config dir");
    fs::create_dir_all(workspace.join("src")).expect("source dir");
    fs::write(workspace.join(".aether/config.toml"), CONFIG).expect("config");
    fs::write(workspace.join("src/ok.rs"), "pub fn unrelated_ok() {}\n").expect("ok");
    fs::write(
        workspace.join("src/secret.rs"),
        if colliding {
            format!("{CLEAN}{DISPLAY}")
        } else {
            CLEAN.to_owned()
        },
    )
    .expect("secret");
}

#[test]
fn legal_collision_does_not_abort_unrelated_indexing_and_is_reported() {
    let temp = tempdir().expect("fixture");
    seed(temp.path(), true);
    run_initial_index_once(&config(temp.path())).expect("quarantine must allow unrelated indexing");
    let store = SqliteStore::open(temp.path()).expect("store");
    assert_eq!(
        store
            .list_symbols_for_file("src/ok.rs")
            .expect("ok symbols")
            .len(),
        1
    );
    assert!(
        store
            .list_symbols_for_file("src/secret.rs")
            .expect("blocked symbols")
            .is_empty()
    );
    let map = assemble_preflight(
        temp.path(),
        &PreflightRequest {
            task: "inspect secret indexing coverage".to_owned(),
            scope: None,
            max_tokens: 2500,
        },
    )
    .expect("preflight");
    assert!(
        map.context
            .contains("1 quarantined identity-collision files")
    );
    assert!(map.context.contains("src/secret.rs"));
    assert!(map.context.contains("Secret::fmt"));
}

#[test]
fn review_regression_first_index_collision_is_explained_in_freshness() {
    let temp = tempdir().expect("fixture");
    seed(temp.path(), true);
    run_initial_index_once(&config(temp.path())).expect("partial index");
    let store = SqliteStore::open(temp.path()).expect("store");
    let freshness =
        aetherd::freshness::assess_index_freshness(temp.path(), &store).expect("freshness");
    assert_eq!(
        freshness.verdict,
        aetherd::freshness::FreshnessVerdict::Stale
    );
    assert!(
        freshness
            .notes
            .iter()
            .any(|note| note.contains("1 quarantined identity-collision files")),
        "{freshness:?}"
    );
}

#[test]
fn collision_keeps_prior_rows_and_sir_then_clears_on_recovery() {
    let temp = tempdir().expect("fixture");
    seed(temp.path(), false);
    run_initial_index_once(&config(temp.path())).expect("initial index");
    let store = SqliteStore::open(temp.path()).expect("store");
    let initial_marker = aetherd::freshness::read_index_sync_marker(temp.path());
    assert!(initial_marker.full_sync_at_ms.is_some());
    let before = store
        .list_symbols_for_file("src/secret.rs")
        .expect("initial rows");
    let method = before
        .iter()
        .find(|row| row.qualified_name == "Secret::fmt")
        .expect("method");
    let payload = r#"{"intent":"keep original secret annotation"}"#;
    store
        .write_sir_blob(&method.id, payload)
        .expect("initial SIR");
    fs::write(
        temp.path().join("src/secret.rs"),
        format!("{CLEAN}{DISPLAY}"),
    )
    .expect("collision");
    fs::write(temp.path().join("src/ok.rs"), "pub fn updated_ok() {}\n").expect("unrelated edit");
    run_initial_index_once(&config(temp.path())).expect("partial index");
    let blocked_marker = aetherd::freshness::read_index_sync_marker(temp.path());
    assert_eq!(
        blocked_marker, initial_marker,
        "quarantine must not publish full-sync freshness"
    );
    let freshness =
        aetherd::freshness::assess_index_freshness(temp.path(), &store).expect("freshness");
    assert_ne!(
        freshness.verdict,
        aetherd::freshness::FreshnessVerdict::Fresh
    );
    let after = store
        .list_symbols_for_file("src/secret.rs")
        .expect("preserved rows");
    assert_eq!(
        before.iter().map(|row| &row.id).collect::<Vec<_>>(),
        after.iter().map(|row| &row.id).collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .read_sir_blob(&method.id)
            .expect("preserved SIR")
            .as_deref(),
        Some(payload)
    );
    assert!(
        store
            .list_symbols_for_file("src/ok.rs")
            .expect("unrelated indexed")
            .iter()
            .any(|row| row.qualified_name == "updated_ok")
    );
    fs::write(temp.path().join("src/secret.rs"), CLEAN).expect("repair source");
    run_initial_index_once(&config(temp.path())).expect("recover quarantine");
    assert!(
        aetherd::indexing_gaps::read_summary(temp.path())
            .expect("receipt")
            .is_none()
    );
    assert_eq!(
        store
            .read_sir_blob(&method.id)
            .expect("SIR after recovery")
            .as_deref(),
        Some(payload)
    );
}

#[test]
fn quarantine_summary_fits_default_budget_and_discloses_unexpanded_paths() {
    let temp = tempdir().expect("fixture");
    seed(temp.path(), false);
    let report = serde_json::json!({
        "version": 1, "snapshot_started_at": 1, "omitted_paths": 0,
        "blocked_sources": (0..128).map(|index| serde_json::json!({
            "path": format!("src/collision_{index}.rs"), "reason": "x".repeat(480)
        })).collect::<Vec<_>>()
    });
    let path = temp.path().join(".aether/indexing-blocked.json");
    let original = serde_json::to_vec(&report).expect("encode");
    fs::write(&path, &original).expect("receipt");
    let map = assemble_preflight(
        temp.path(),
        &PreflightRequest {
            task: "inspect indexing coverage".to_owned(),
            scope: None,
            max_tokens: 2500,
        },
    )
    .expect("bounded quarantine summary");
    assert!(map.estimated_tokens <= 2500);
    assert!(
        map.context
            .contains("128 quarantined identity-collision files")
    );
    assert!(map.context.contains("124 additional blocked paths"));
    assert_eq!(fs::read(path).expect("receipt after read"), original);
}

#[test]
fn overflowing_quarantine_count_returns_a_diagnostic_error() {
    let temp = tempdir().expect("fixture");
    seed(temp.path(), false);
    let report = serde_json::json!({
        "version": 1, "snapshot_started_at": 1, "omitted_paths": usize::MAX,
        "blocked_sources": [{"path": "src/secret.rs", "reason": "collision"}]
    });
    fs::write(
        temp.path().join(".aether/indexing-blocked.json"),
        serde_json::to_vec(&report).expect("encode"),
    )
    .expect("receipt");
    let result = aetherd::indexing_gaps::read_summary(temp.path());
    assert!(result.is_err(), "untrusted counts must not wrap or panic");
}
