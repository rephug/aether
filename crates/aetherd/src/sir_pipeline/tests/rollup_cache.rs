use super::*;

struct Fixture {
    _temp: tempfile::TempDir,
    store: SqliteStore,
    pipeline: SirPipeline,
    calls: Arc<AtomicUsize>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempdir().expect("tempdir");
        write_embeddings_only_config(temp.path());
        let store = SqliteStore::open(temp.path()).expect("store");
        for i in 0..6 {
            let id = format!("leaf-{i}");
            store
                .upsert_symbol(demo_symbol_record_with_kind(
                    &id,
                    &id,
                    "function",
                    "src/lib.rs",
                ))
                .expect("symbol");
            store
                .write_sir_blob(&id, &canonicalize_sir_json(&demo_sir()))
                .expect("leaf SIR");
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let pipeline = build_write_pipeline(
            temp.path(),
            Arc::new(CountingInferenceProvider {
                symbol_sir: demo_sir(),
                prompt_sir: demo_rollup_sir(),
                standard_calls: Arc::new(AtomicUsize::new(0)),
                prompt_calls: Arc::new(AtomicUsize::new(0)),
                file_calls: calls.clone(),
            }),
        );
        Self {
            _temp: temp,
            store,
            pipeline,
            calls,
        }
    }
    fn refresh(&self, bulk: bool, pass: &str) {
        self.refresh_forced(bulk, pass, false);
    }
    fn refresh_forced(&self, bulk: bool, pass: &str, force: bool) {
        let mut out = Vec::new();
        if bulk {
            self.pipeline
                .bulk_upsert_file_rollups(
                    &self.store,
                    BTreeMap::from([("src/lib.rs".to_owned(), Language::Rust)]),
                    force,
                    false,
                    &mut out,
                    None,
                    pass,
                )
                .expect("bulk refresh");
        } else {
            self.pipeline
                .upsert_file_rollup(
                    &self.store,
                    "src/lib.rs",
                    Language::Rust,
                    force,
                    false,
                    &mut out,
                    None,
                    pass,
                )
                .expect("refresh");
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[test]
fn unchanged_rollups_skip_provider_in_single_and_bulk_paths() {
    let mut f = Fixture::new();
    f.refresh(false, "scan");
    for _ in 0..20 {
        f.refresh(true, "scan");
        f.refresh(false, "scan");
    }
    assert_eq!(
        f.calls(),
        1,
        "forty refreshes reuse the verified persisted output"
    );
    // Legacy/new instances have no trusted local cache, even if metadata matches.
    f.pipeline = build_write_pipeline(f._temp.path(), f.pipeline.provider.clone());
    f.refresh(true, "scan");
    assert_eq!(f.calls(), 2);
    for _ in 0..40 {
        f.pipeline.rollup_cache.lock().expect("cache").clear();
        f.refresh(false, "scan");
    }
    assert_eq!(
        f.calls(),
        42,
        "uncached control calls provider on every refresh"
    );
}

#[test]
fn rollup_cache_invalidates_for_changed_leaf_pass_model_and_timeout() {
    let mut f = Fixture::new();
    f.refresh(false, "scan");
    let mut changed = demo_sir();
    changed.confidence = 0.2; // Not just the textual intent: full leaf output matters.
    f.store
        .write_sir_blob("leaf-0", &canonicalize_sir_json(&changed))
        .expect("change");
    f.refresh(true, "scan");
    assert_eq!(f.calls(), 2);
    f.refresh(false, "deep");
    assert_eq!(f.calls(), 3);
    f.pipeline.model_name = "other-model".to_owned();
    f.refresh(false, "deep");
    assert_eq!(f.calls(), 4);
    f.pipeline.inference_timeout_secs += 1;
    f.refresh(false, "deep");
    assert_eq!(f.calls(), 5);
}

#[test]
fn rollup_cache_rejects_stale_metadata_and_replaced_blob() {
    let f = Fixture::new();
    let id = synthetic_file_sir_id("rust", "src/lib.rs");
    f.refresh(false, "scan");
    f.store
        .mark_sir_stale(std::slice::from_ref(&id))
        .expect("mark stale");
    f.refresh(false, "scan");
    assert_eq!(f.calls(), 2);
    f.store.write_sir_blob(&id, "{}").expect("replace blob");
    f.refresh(true, "scan");
    assert_eq!(
        f.calls(),
        3,
        "corrupt or externally replaced output cannot hit cache"
    );
    let blob = f.store.read_sir_blob(&id).expect("blob").expect("repaired");
    let _: FileSir = serde_json::from_str(&blob).expect("valid repaired rollup");
    f.refresh(false, "scan");
    assert_eq!(f.calls(), 3, "repaired rollup is reusable");
}

#[test]
fn stale_rollup_persist_cannot_seed_cache() {
    let f = Fixture::new();
    let old = f
        .pipeline
        .load_file_rollup_leaf_sirs(&f.store, "src/lib.rs")
        .expect("leaves");
    let fingerprint = super::super::file_rollups::leaf_fingerprint(&old);
    let output = concatenate_file_sir(&old);
    let mut changed = demo_sir();
    changed.intent = "new body".to_owned();
    f.store
        .write_sir_blob("leaf-0", &canonicalize_sir_json(&changed))
        .expect("change");
    f.pipeline
        .persist_file_rollup(
            &f.store,
            "src/lib.rs",
            Language::Rust,
            &output,
            Some(&fingerprint),
            false,
            &mut Vec::new(),
            None,
            "scan",
        )
        .expect("rejected persist");
    assert!(f.pipeline.rollup_cache.lock().expect("cache").is_empty());
    f.refresh(true, "scan");
    assert_eq!(f.calls(), 1);
}

#[test]
fn fallback_rollups_do_not_pin_provider_failures() {
    let f = Fixture::new();
    let leaves = f
        .pipeline
        .load_file_rollup_leaf_sirs(&f.store, "src/lib.rs")
        .expect("leaves");
    let fingerprint = super::super::file_rollups::leaf_fingerprint(&leaves);
    let fallback = concatenate_file_sir(&leaves);
    f.pipeline
        .persist_file_rollup(
            &f.store,
            "src/lib.rs",
            Language::Rust,
            &fallback,
            Some(&fingerprint),
            false,
            &mut Vec::new(),
            None,
            "scan",
        )
        .expect("fallback");
    assert!(f.pipeline.rollup_cache.lock().expect("cache").is_empty());
    f.refresh(false, "scan");
    assert_eq!(f.calls(), 1);
}

#[test]
fn forced_rollups_bypass_cache_in_single_and_bulk_paths() {
    let f = Fixture::new();
    f.refresh(false, "scan");
    f.refresh_forced(false, "scan", true);
    f.refresh_forced(true, "scan", true);
    assert_eq!(f.calls(), 3);
    f.refresh(false, "scan");
    assert_eq!(f.calls(), 3);
}

#[test]
fn removed_leaf_rebuilds_rollup_without_old_export() {
    let f = Fixture::new();
    f.refresh(false, "scan");
    f.store.mark_removed("leaf-0").expect("retire leaf");
    f.refresh(true, "scan");
    let id = synthetic_file_sir_id("rust", "src/lib.rs");
    let blob = f.store.read_sir_blob(&id).expect("blob").expect("rollup");
    let sir: FileSir = serde_json::from_str(&blob).expect("file SIR");
    assert_eq!(sir.symbol_count, 5);
    assert!(!sir.exports.contains(&"leaf-0".to_owned()));
    assert_eq!(f.calls(), 1, "five leaves use local aggregation");
}
