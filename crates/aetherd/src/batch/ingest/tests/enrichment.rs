//! Real batch build/ingest cycles, with fixed embeddings and synthesized responses.
//! No provider submission, inference request, local model or network is used.

use super::*;
use crate::batch::BatchRuntimeConfig;
use crate::batch::build::{BuildSummary, build_pass_jsonl_for_ids};
use crate::batch::gemini::GeminiBatchProvider;

struct OfflineBatchProvider(GeminiBatchProvider);

#[async_trait]
impl BatchProvider for OfflineBatchProvider {
    fn cache_context(&self) -> serde_json::Value {
        self.0.cache_context()
    }
    fn format_request(
        &self,
        key: &str,
        system: &str,
        user: &str,
        model: &str,
        thinking: &str,
    ) -> Result<String> {
        self.0.format_request(key, system, user, model, thinking)
    }
    fn parse_result_line(&self, line: &str) -> Result<BatchResultLine> {
        self.0.parse_result_line(line)
    }
    fn name(&self) -> &str {
        self.0.name()
    }
    async fn submit(&self, _: &Path, _: &str, _: &Path, _: u64) -> Result<Vec<String>> {
        panic!("offline regression must never submit a model request")
    }
    async fn poll(&self, _: &[String]) -> Result<BatchPollStatus> {
        panic!("offline test must not poll")
    }
    async fn download_results(&self, _: &[String], _: &Path) -> Result<Vec<PathBuf>> {
        panic!("offline test must not download results")
    }
}

struct Fixture {
    store: SqliteStore,
    pipeline: SirPipeline,
    config: AetherConfig,
    pass: PassConfig,
    runtime: BatchRuntimeConfig,
    symbols: HashMap<String, Symbol>,
    id: String,
    provider: GeminiBatchProvider,
    // Close database handles before removing the workspace on Windows.
    temp: tempfile::TempDir,
}

impl Fixture {
    fn new(pass: BatchPass) -> Self {
        let temp = tempdir().expect("synthetic workspace");
        let workspace = temp.path();
        write_embeddings_only_config(workspace);
        let config = aether_config::load_workspace_config(workspace).expect("config");
        let store = SqliteStore::open(workspace).expect("synthetic store");
        let pipeline = SirPipeline::new_embeddings_only_with(
            workspace.to_path_buf(),
            std::sync::Arc::new(FixedEmbeddingProvider),
            "test_embedding",
            "test-model",
            None,
        )
        .expect("fixed embedding pipeline")
        .with_skip_surreal_sync(true);
        let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", "fn run() -> i32 { 1 }\n");
        store.upsert_symbol(record).expect("catalog");
        let id = symbol.id.clone();
        let mut pass_config = triage_pass_config();
        pass_config.pass = pass;
        pass_config.max_chars = 8;
        let runtime = BatchRuntimeConfig {
            batch_dir: workspace.join("batch"),
            jsonl_chunk_size: 10,
            poll_interval_secs: 1,
            max_symbols: 0,
            max_concurrent_jobs: 1,
            scan: pass_config.clone(),
            triage: pass_config.clone(),
            deep: pass_config.clone(),
        };
        let fixture = Self {
            temp,
            store,
            pipeline,
            config,
            pass: pass_config,
            runtime,
            symbols: HashMap::from([(id.clone(), symbol)]),
            id,
            provider: GeminiBatchProvider::new("unused-offline-fixture-key".into()),
        };
        fixture.persist(demo_sir(), "scan", None);
        fixture
    }

    fn persist(&self, sir: SirAnnotation, pass: &str, prompt_hash: Option<String>) {
        self.pipeline
            .persist_sir_payload_into_sqlite(
                &self.store,
                &UpsertSirIntentPayload {
                    symbol: self.symbols[&self.id].clone(),
                    sir,
                    provider_name: "gemini".into(),
                    model_name: self.pass.model.clone(),
                    generation_pass: pass.into(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash,
                    source_hash: Some(self.symbols[&self.id].content_hash.clone()),
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("persist fixture SIR");
    }

    fn build(&self) -> BuildSummary {
        build_pass_jsonl_for_ids(
            self.temp.path(),
            &self.store,
            &self.runtime,
            &self.pass,
            &self.symbols,
            Some(std::slice::from_ref(&self.id)),
            false,
            &self.provider,
        )
        .expect("offline batch build")
    }

    fn ingest(&self, build: &BuildSummary, intent: &str) -> IngestSummary {
        self.ingest_with_pipeline(build, intent, &self.pipeline)
            .expect("offline production ingest")
    }

    fn ingest_with_pipeline(
        &self,
        build: &BuildSummary,
        intent: &str,
        pipeline: &SirPipeline,
    ) -> Result<IngestSummary> {
        let key = build.origins.keys().next().expect("one request");
        let mut sir = demo_sir();
        sir.intent = intent.into();
        let response = serde_json::json!({
            "key": key,
            "response": {"candidates": [{"content": {"parts": [
                {"text": serde_json::to_string(&sir).expect("SIR JSON")}
            ]}}]}
        });
        let results = self.runtime.batch_dir.join(format!(
            "{}.{}.results.jsonl",
            self.pass.pass.as_str(),
            build.build_id
        ));
        fs::write(&results, format!("{response}\n")).expect("synthesized result");
        ingest_results_with_pipeline(
            self.temp.path(),
            &self.store,
            &self.pass,
            &results,
            &self.config,
            &self.provider,
            "gemini",
            || Ok(pipeline),
        )
    }

    fn monitor_config(&self) -> AetherConfig {
        let mut config = self.config.clone();
        config.batch = Some(aether_config::BatchConfig {
            provider: "gemini".into(),
            scan_model: self.pass.model.clone(),
            triage_model: self.pass.model.clone(),
            deep_model: self.pass.model.clone(),
            scan_thinking: self.pass.thinking.clone(),
            triage_thinking: self.pass.thinking.clone(),
            deep_thinking: self.pass.thinking.clone(),
            triage_neighbor_depth: self.pass.neighbor_depth,
            deep_neighbor_depth: self.pass.neighbor_depth,
            scan_max_chars: self.pass.max_chars,
            triage_max_chars: self.pass.max_chars,
            deep_max_chars: self.pass.max_chars,
            prompt_tier: self.pass.prompt_tier.clone(),
            batch_dir: self.runtime.batch_dir.to_string_lossy().into_owned(),
            ..Default::default()
        });
        config.continuous = Some(aether_config::ContinuousConfig {
            auto_submit: false,
            requeue_pass: self.pass.pass.as_str().into(),
            max_requeue_per_run: 1,
            ..Default::default()
        });
        config
    }

    fn monitor(&self, config: &AetherConfig) -> crate::continuous::ContinuousStatusSnapshot {
        crate::continuous::run_monitor_once_with_provider(
            self.temp.path(),
            config,
            1_700_000_000,
            || {
                Ok(Box::new(OfflineBatchProvider(GeminiBatchProvider::new(
                    "unused".into(),
                ))))
            },
        )
        .expect("offline continuous selection/build")
    }
}

fn assert_stable_enrichment_stops(pass: BatchPass) {
    let fixture = Fixture::new(pass);
    let mut requests = Vec::new();
    let mut fingerprints = Vec::new();
    for cycle in 0..4 {
        let build = fixture.build();
        requests.push(build.written);
        if build.written > 0 {
            let (_, fingerprint) =
                parse_key(build.origins.keys().next().expect("key")).expect("request identity");
            fingerprints.push(fingerprint.to_owned());
            let ingested = fixture.ingest(&build, &format!("Revised explanation {cycle}"));
            assert_eq!(
                (ingested.processed, ingested.skipped, ingested.superseded),
                (1, 0, 0)
            );
        }
    }
    eprintln!("unchanged-source requests: {requests:?}; fingerprints: {fingerprints:?}");
    assert_eq!(
        requests,
        [1, 0, 0, 0],
        "accepted output must not schedule itself again"
    );
}

#[test]
fn repeated_triage_does_not_enrich_its_own_result() {
    assert_stable_enrichment_stops(BatchPass::Triage);
}

#[test]
fn repeated_deep_does_not_enrich_its_own_result() {
    assert_stable_enrichment_stops(BatchPass::Deep);
}

#[test]
fn continuous_reselection_of_a_fresh_symbol_does_not_rebuild_its_completed_enrichment() {
    let fixture = Fixture::new(BatchPass::Triage);
    let config = fixture.monitor_config();
    let first = fixture.monitor(&config);
    assert_eq!((first.selected_symbols, first.written_requests), (1, 1));
    let keymap = fs::read_dir(&fixture.runtime.batch_dir)
        .expect("batch directory")
        .map(|e| e.expect("entry").path())
        .find(|p| {
            p.file_name()
                .expect("name")
                .to_string_lossy()
                .ends_with(".origin.json")
        })
        .expect("monitor origin");
    let origins: HashMap<String, BatchRequestOrigin> =
        serde_json::from_slice(&fs::read(&keymap).expect("origin bytes")).expect("origins");
    let key = origins.keys().next().expect("request");
    let build = BuildSummary {
        files: Vec::new(),
        written: 1,
        skipped: 0,
        unresolved_symbols: 0,
        keymap: HashMap::new(),
        build_id: crate::batch::key::key_build_id(key)
            .expect("build id")
            .to_owned(),
        origins,
    };
    assert_eq!(
        fixture
            .ingest(&build, "Completed continuous enrichment")
            .processed,
        1
    );
    for _ in 0..3 {
        let next = fixture.monitor(&config);
        assert_eq!(
            (
                next.selected_symbols,
                next.written_requests,
                next.skipped_requests
            ),
            (1, 0, 1)
        );
        assert!(!next.auto_submit);
    }
    let mut disabled = config;
    disabled
        .continuous
        .as_mut()
        .expect("continuous")
        .max_requeue_per_run = 0;
    let bounded = fixture.monitor(&disabled);
    assert_eq!((bounded.selected_symbols, bounded.written_requests), (0, 0));
}

#[test]
fn completion_does_not_rewrite_the_actual_request_fingerprint() {
    let fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    let key = first.origins.keys().next().expect("request");
    let request_hash = parse_key(key).expect("key").1.to_owned();
    fixture.ingest(&first, "Accepted result");
    assert_eq!(
        fixture
            .store
            .get_sir_meta(&fixture.id)
            .expect("meta")
            .expect("row")
            .prompt_hash
            .as_deref(),
        Some(request_hash.as_str())
    );
    assert_eq!(fixture.build().written, 0);
    let receipt =
        crate::batch::completion::path(fixture.temp.path(), fixture.pass.pass, &fixture.id);
    fs::remove_file(receipt).expect("remove only the synthetic fixture receipt");
    let next = fixture.build();
    assert_eq!(
        next.written, 1,
        "cache loss conservatively permits a new request"
    );
    let next_hash = parse_key(next.origins.keys().next().expect("request"))
        .expect("key")
        .1;
    assert_ne!(
        next_hash, request_hash,
        "the new baseline still changes full request identity"
    );
}

#[test]
fn a_same_content_rewrite_invalidates_the_completion_by_write_generation() {
    let fixture = Fixture::new(BatchPass::Deep);
    let first = fixture.build();
    fixture.ingest(&first, "Accepted result");
    assert_eq!(fixture.build().written, 0);
    let before = current_sir_identity(&fixture.store, &fixture.id)
        .expect("identity")
        .expect("row");
    let meta = fixture
        .store
        .get_sir_meta(&fixture.id)
        .expect("meta")
        .expect("row");
    let sir: SirAnnotation = serde_json::from_str(
        &fixture
            .store
            .read_sir_blob(&fixture.id)
            .expect("blob")
            .expect("SIR"),
    )
    .expect("annotation");
    // Even retaining all provenance fields cannot make another write the same write.
    fixture.persist(sir, "deep", meta.prompt_hash);
    let after = current_sir_identity(&fixture.store, &fixture.id)
        .expect("identity")
        .expect("row");
    assert_eq!(
        (before.sir_hash, before.sir_version),
        (after.sir_hash, after.sir_version)
    );
    assert_ne!(before.write_generation, after.write_generation);
    assert_eq!(fixture.build().written, 1);
}

#[test]
fn settings_changes_still_require_a_request_after_completion() {
    let mut fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    fixture.ingest(&first, "Accepted result");
    let original = fixture.pass.clone();
    for change in 0..5 {
        fixture.pass = original.clone();
        match change {
            0 => fixture.pass.model.push_str("-changed"),
            1 => fixture.pass.thinking = "high".into(),
            2 => fixture.pass.prompt_tier = "full".into(),
            3 => fixture.pass.max_chars += 1,
            _ => fixture.pass.neighbor_depth += 1,
        }
        assert_eq!(fixture.build().written, 1, "settings change {change}");
    }
    fixture.pass = original;
    assert_eq!(fixture.build().written, 0);
}

#[test]
fn full_source_tail_changes_still_require_enrichment() {
    let mut fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    fixture.ingest(&first, "Accepted result");
    let (edited, _) = parsed_symbol(fixture.temp.path(), "src/lib.rs", "fn run() -> i32 { 2 }\n");
    assert_eq!(edited.id, fixture.id);
    fixture.symbols.insert(edited.id.clone(), edited);
    let next = fixture.build();
    assert_eq!(
        next.written, 1,
        "the eight-byte excerpt is unchanged, full source differs"
    );
    fixture.ingest(&next, "Result for new source");
    assert_eq!(fixture.build().written, 0);
}

struct FailingEmbeddingProvider;

#[async_trait]
impl aether_infer::EmbeddingProvider for FailingEmbeddingProvider {
    async fn embed_text(&self, _: &str) -> Result<Vec<f32>, aether_infer::InferError> {
        Err(aether_infer::InferError::InvalidEmbeddingResponse(
            "deliberate offline fixture failure".into(),
        ))
    }
}

#[test]
fn failed_downstream_work_does_not_get_a_receipt_and_can_resume_without_regeneration() {
    let fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    let failing = SirPipeline::new_embeddings_only_with(
        fixture.temp.path().to_path_buf(),
        std::sync::Arc::new(FailingEmbeddingProvider),
        "test_embedding",
        "test-model",
        None,
    )
    .expect("failing fixture pipeline")
    .with_skip_surreal_sync(true);
    assert!(
        fixture
            .ingest_with_pipeline(&first, "Accepted leaf, failed embedding", &failing)
            .is_err()
    );
    let receipt =
        crate::batch::completion::path(fixture.temp.path(), fixture.pass.pass, &fixture.id);
    assert!(!receipt.exists());
    assert_eq!(
        fixture.build().written,
        1,
        "failed work must not be acknowledged as complete"
    );
    let resumed = fixture.ingest(&first, "Accepted leaf, failed embedding");
    assert_eq!((resumed.processed, resumed.resumed), (1, 1));
    assert_eq!(fixture.build().written, 0);
}

#[test]
fn superseded_results_do_not_acknowledge_or_suppress_the_new_baseline() {
    let fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    let mut manual = demo_sir();
    manual.intent = "Manually corrected baseline".into();
    fixture.persist(manual, "scan", None);
    let superseded = fixture.ingest(&first, "Late result for old baseline");
    assert_eq!((superseded.processed, superseded.superseded), (0, 1));
    assert!(
        !crate::batch::completion::path(fixture.temp.path(), fixture.pass.pass, &fixture.id)
            .exists()
    );
    assert_eq!(fixture.build().written, 1);
}

#[test]
fn delayed_completion_cannot_replace_a_newer_writers_receipt() {
    let fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    fixture.ingest(&first, "First accepted result");
    let first_identity = current_sir_identity(&fixture.store, &fixture.id)
        .expect("first identity")
        .expect("first row");
    fixture.persist(demo_sir(), "scan", None);
    let second = fixture.build();
    fixture.ingest(&second, "Newer accepted result");
    let receipt =
        crate::batch::completion::path(fixture.temp.path(), fixture.pass.pass, &fixture.id);
    let newer_receipt = fs::read(&receipt).expect("newer completion");

    // Simulate the first ingest reaching publication after another writer has
    // already replaced its leaf and published a completion for that replacement.
    let (key, origin) = first.origins.iter().next().expect("first request");
    crate::batch::completion::record(
        fixture.temp.path(),
        &fixture.store,
        fixture.pass.pass,
        key,
        &first_identity,
        origin
            .enrichment_baseline
            .as_deref()
            .expect("first baseline"),
    )
    .expect("superseded publication is harmless");
    assert_eq!(fs::read(receipt).expect("completion"), newer_receipt);
    assert_eq!(fixture.build().written, 0);
}

#[test]
fn corrupt_completion_data_never_suppresses_a_request() {
    let fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    fixture.ingest(&first, "Accepted result");
    let receipt =
        crate::batch::completion::path(fixture.temp.path(), fixture.pass.pass, &fixture.id);
    fs::write(receipt, "{incomplete").expect("corrupt only synthetic cache");
    assert_eq!(fixture.build().written, 1);
}

#[test]
fn changed_file_context_still_requires_a_request() {
    let fixture = Fixture::new(BatchPass::Triage);
    let first = fixture.build();
    fixture.ingest(&first, "Accepted result");
    let file_id = aether_sir::synthetic_file_sir_id("rust", "src/lib.rs");
    let file = aether_sir::FileSir {
        intent: "New file context".into(),
        exports: vec![],
        side_effects: vec![],
        dependencies: vec![],
        error_modes: vec![],
        symbol_count: 1,
        confidence: 0.9,
    };
    fixture
        .store
        .write_sir_blob(&file_id, &serde_json::to_string(&file).expect("file SIR"))
        .expect("file context");
    let next = fixture.build();
    assert_eq!(next.written, 1);
    fixture.ingest(&next, "Result using changed context");
    assert_eq!(fixture.build().written, 0);
}

#[test]
fn legacy_origins_ingest_normally_without_inventing_a_completion() {
    let fixture = Fixture::new(BatchPass::Triage);
    let mut first = fixture.build();
    for origin in first.origins.values_mut() {
        origin.enrichment_baseline = None;
    }
    let origin_path = fixture
        .runtime
        .batch_dir
        .join(crate::batch::build::origin_sidecar_name(
            fixture.pass.pass.as_str(),
            &first.build_id,
        ));
    fs::write(
        origin_path,
        serde_json::to_vec(&first.origins).expect("legacy origin JSON"),
    )
    .expect("legacy sidecar");
    assert_eq!(fixture.ingest(&first, "Legacy result").processed, 1);
    assert!(
        !crate::batch::completion::path(fixture.temp.path(), fixture.pass.pass, &fixture.id)
            .exists()
    );
    let next = fixture.build();
    assert_eq!(
        next.written, 1,
        "no provenance is fabricated for an older sidecar"
    );
    fixture.ingest(&next, "New result with completion provenance");
    assert_eq!(fixture.build().written, 0);
}

#[test]
fn changed_neighbor_summary_still_requires_a_request() {
    use aether_store::SymbolRelationStore;
    let mut fixture = Fixture::new(BatchPass::Triage);
    let (peer, record) = parsed_symbol(fixture.temp.path(), "src/peer.rs", "fn peer() {}\n");
    fixture.store.upsert_symbol(record).expect("peer catalog");
    let persist_peer = |intent: &str| {
        let mut sir = demo_sir();
        sir.intent = intent.into();
        fixture
            .pipeline
            .persist_sir_payload_into_sqlite(
                &fixture.store,
                &UpsertSirIntentPayload {
                    symbol: peer.clone(),
                    sir,
                    provider_name: "gemini".into(),
                    model_name: "peer-model".into(),
                    generation_pass: "scan".into(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash: None,
                    source_hash: Some(peer.content_hash.clone()),
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("peer SIR");
    };
    persist_peer("Original neighbor summary");
    fixture
        .store
        .upsert_edges(&[aether_core::SymbolEdge {
            target_symbol_id: Some(peer.id.clone()),
            target_file_path: None,
            source_id: fixture.id.clone(),
            target_qualified_name: peer.qualified_name.clone(),
            edge_kind: aether_core::EdgeKind::Calls,
            file_path: "src/lib.rs".into(),
        }])
        .expect("dependency edge");
    fixture.symbols.insert(peer.id.clone(), peer.clone());
    let first = fixture.build();
    fixture.ingest(&first, "Accepted result");
    assert_eq!(fixture.build().written, 0);
    persist_peer("Corrected neighbor summary");
    let next = fixture.build();
    assert_eq!(
        next.written, 1,
        "context changes are not hidden by a completion"
    );
    fixture.ingest(&next, "Result using corrected neighbor");
    assert_eq!(fixture.build().written, 0);
}
