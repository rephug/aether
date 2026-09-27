//! Batch ingest tests: result parsing and key handling, the superseded-result skips,
//! the resumed retry of a downstream-failed result and its once-per-result fingerprint.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aether_store::{SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord};
use async_trait::async_trait;
use tempfile::tempdir;

use super::*;
use crate::batch::{BatchPollStatus, BatchProvider, BatchResultLine};
use crate::cli::BatchPass;

struct StubBatchProvider {
    key: String,
    text: String,
    reasoning_trace: Option<String>,
}

#[async_trait]
impl BatchProvider for StubBatchProvider {
    fn format_request(
        &self,
        _key: &str,
        _system_prompt: &str,
        _user_prompt: &str,
        _model: &str,
        _thinking: &str,
    ) -> Result<String> {
        unreachable!("format_request is not used in ingest tests")
    }

    async fn submit(
        &self,
        _input_path: &Path,
        _model: &str,
        _batch_dir: &Path,
        _poll_interval_secs: u64,
    ) -> Result<Vec<String>> {
        unreachable!("submit is not used in ingest tests")
    }

    async fn poll(&self, _job_ids: &[String]) -> Result<BatchPollStatus> {
        unreachable!("poll is not used in ingest tests")
    }

    async fn download_results(
        &self,
        _job_ids: &[String],
        _output_dir: &Path,
    ) -> Result<Vec<PathBuf>> {
        unreachable!("download_results is not used in ingest tests")
    }

    fn parse_result_line(&self, _line: &str) -> Result<BatchResultLine> {
        Ok(BatchResultLine::Success {
            key: self.key.clone(),
            text: self.text.clone(),
            reasoning_trace: self.reasoning_trace.clone(),
        })
    }

    fn name(&self) -> &str {
        "stub"
    }
}

fn write_embeddings_only_config(workspace: &Path) {
    fs::create_dir_all(workspace.join(".aether")).expect("create .aether");
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )
    .expect("write config");
}

fn demo_symbol_record(symbol_id: &str, qualified_name: &str) -> SymbolRecord {
    SymbolRecord {
        id: symbol_id.to_owned(),
        file_path: "src/lib.rs".to_owned(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{symbol_id}"),
        last_seen_at: 1_700_000_000,
    }
}

/// Writes `source` to `file_path` in the workspace, parses it and returns its one symbol
/// with the catalog record the store keeps for it. The ids are the parser's, so a writer
/// re-parsing the file under the inject lock finds the symbol by id.
fn parsed_symbols(workspace: &Path, file_path: &str, source: &str) -> Vec<Symbol> {
    let path = workspace.join(file_path);
    fs::create_dir_all(path.parent().expect("file has a parent")).expect("create parent");
    fs::write(&path, source).expect("write source");
    let mut extractor = aether_parse::SymbolExtractor::new().expect("initialize parser");
    extractor
        .extract_from_path(Path::new(file_path), source)
        .expect("parse source")
}

fn parsed_symbol(
    workspace: &Path,
    file_path: &str,
    source: &str,
) -> (Symbol, aether_store::SymbolRecord) {
    let mut symbols = parsed_symbols(workspace, file_path, source);
    assert_eq!(symbols.len(), 1, "the source declares one symbol");
    let symbol = symbols.remove(0);
    let record = aether_store::SymbolRecord {
        id: symbol.id.clone(),
        file_path: symbol.file_path.clone(),
        language: "rust".to_owned(),
        kind: "function".to_owned(),
        qualified_name: symbol.qualified_name.clone(),
        signature_fingerprint: symbol.signature_fingerprint.clone(),
        last_seen_at: 1_700_000_000,
    };
    (symbol, record)
}

fn triage_pass_config() -> PassConfig {
    PassConfig {
        pass: BatchPass::Triage,
        model: "triage-model".to_owned(),
        thinking: "low".to_owned(),
        neighbor_depth: 1,
        max_chars: 8_000,
        prompt_tier: "standard".to_owned(),
    }
}

fn demo_sir() -> SirAnnotation {
    SirAnnotation {
        intent: "Demo intent".to_owned(),
        behavior: None,
        inputs: vec!["input".to_owned()],
        outputs: vec!["output".to_owned()],
        side_effects: Vec::new(),
        dependencies: Vec::new(),
        error_modes: Vec::new(),
        confidence: 0.9,
        edge_cases: None,
        complexity: None,
        method_dependencies: None,
    }
}

#[test]
fn parse_key_reads_two_and_three_part_keys() {
    assert_eq!(parse_key("sym|hash").expect("two parts"), ("sym", "hash"));
    assert_eq!(
        parse_key("sym|hash|0123456789ab").expect("three parts"),
        ("sym", "hash")
    );
    assert!(parse_key("|hash").is_err());
    assert!(parse_key("sym|").is_err());
    assert!(key_has_build_id("sym|hash|0123456789ab"));
    assert!(!key_has_build_id("sym|hash"));
    assert!(!key_has_build_id("sym|hash|"));
    assert!(!key_has_build_id("sym"));
}

#[test]
fn prepare_symbol_skips_a_result_whose_sir_moved_on_since_the_build() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
        .map(|pipeline| pipeline.with_skip_surreal_sync(true))
        .expect("build embeddings-only pipeline");
    // The symbol's source on disk, as the build saw it.
    let source = "fn late() {}\n";
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", source);
    store.upsert_symbol(record).expect("upsert symbol");
    let id = symbol.id.as_str();
    let persist = |sir: &SirAnnotation, pass: &str| {
        pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol.clone(),
                    sir: sir.clone(),
                    provider_name: "gemini".to_owned(),
                    model_name: "scan-model".to_owned(),
                    generation_pass: pass.to_owned(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash: None,
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("persist payload");
    };

    // The batch was built while the store held the scan SIR...
    persist(&demo_sir(), "scan");
    let built_against = current_sir_identity(&store, id).expect("identity");
    let key = format!("{id}|prompt-late|build-1");
    let source_hash = symbol.content_hash.clone();
    let origins = HashMap::from([(
        key.clone(),
        BatchRequestOrigin {
            prior_sir: built_against,
            source_hash: source_hash.clone(),
        },
    )]);

    // ...and an injection replaced it before the result came back.
    let reviewed = SirAnnotation {
        intent: "Reviewed by hand".to_owned(),
        confidence: 0.97,
        ..demo_sir()
    };
    persist(&reviewed, "injected");
    let reviewed_identity = current_sir_identity(&store, id).expect("identity");

    let batch_sir = SirAnnotation {
        intent: "Triage result from the older state".to_owned(),
        ..demo_sir()
    };
    let provider = StubBatchProvider {
        key: key.clone(),
        text: serde_json::to_string(&batch_sir).expect("serialize sir"),
        reasoning_trace: None,
    };
    let outcome = prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider,
        "gemini",
        &HashMap::new(),
        &origins,
    )
    .expect("prepare symbol");
    assert!(
        outcome.is_none(),
        "a result for a replaced SIR is not applied"
    );
    let meta = store
        .get_sir_meta(id)
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(
        current_sir_identity(&store, id).expect("identity"),
        reviewed_identity,
        "the injected SIR must stand"
    );
    assert_eq!(meta.generation_pass, "injected");
    assert_eq!(
        meta.prompt_hash, None,
        "no provenance from the skipped result"
    );

    // Built against the SIR the store still holds but a source since edited on
    // disk (same id, different body), the result is not applied either: the
    // daemon's regeneration from the new source must not be pre-empted by a SIR
    // of the old.
    let origins = HashMap::from([(
        key.clone(),
        BatchRequestOrigin {
            prior_sir: reviewed_identity.clone(),
            source_hash: source_hash.clone(),
        },
    )]);
    fs::write(workspace.join("src/lib.rs"), "fn late() { edited }\n").expect("edit source");
    let outcome = prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider,
        "gemini",
        &HashMap::new(),
        &origins,
    )
    .expect("prepare symbol");
    assert!(
        outcome.is_none(),
        "a result for an edited source is not applied"
    );
    assert_eq!(
        store
            .get_sir_meta(id)
            .expect("load sir meta")
            .expect("sir meta exists")
            .generation_pass,
        "injected"
    );

    // An edit elsewhere in the file moves the symbol without touching its body: the
    // re-parse under the lock finds it by id at its new position, so the result
    // below still applies rather than reading a shifted range as a change.
    fs::write(
        workspace.join("src/lib.rs"),
        format!("fn other() {{}}\n\n{source}"),
    )
    .expect("shift source");

    // A result carrying a build id but no origin entry is refused, not ingested
    // unchecked.
    let err = match prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider,
        "gemini",
        &HashMap::new(),
        &HashMap::new(),
    ) {
        Err(err) => err,
        Ok(_) => panic!("a modern key without an origin must be refused"),
    };
    assert!(
        err.to_string()
            .contains("has no entry in its build's origin sidecar"),
        "unexpected error: {err:#}"
    );
    assert_eq!(
        store
            .get_sir_meta(id)
            .expect("load sir meta")
            .expect("sir meta exists")
            .generation_pass,
        "injected"
    );

    // Built against the SIR the workspace still holds and a body it still has, only
    // moved by the edit above, the result is applied.
    let outcome = prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider,
        "gemini",
        &HashMap::new(),
        &origins,
    )
    .expect("prepare symbol");
    assert!(outcome.is_some());
    let meta = store
        .get_sir_meta(id)
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.sir_hash, aether_sir::sir_hash(&batch_sir));
    assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-late"));
}

#[test]
fn prepare_symbol_resumes_a_result_an_earlier_attempt_already_persisted() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
        .map(|pipeline| pipeline.with_skip_surreal_sync(true))
        .expect("build embeddings-only pipeline");
    let source = "fn retry() {}\n";
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", source);
    store.upsert_symbol(record).expect("upsert symbol");
    let id = symbol.id.as_str();
    let persist = |sir: &SirAnnotation, pass: &str| {
        pipeline
            .persist_sir_payload_into_sqlite(
                &store,
                &UpsertSirIntentPayload {
                    symbol: symbol.clone(),
                    sir: sir.clone(),
                    provider_name: "gemini".to_owned(),
                    model_name: "scan-model".to_owned(),
                    generation_pass: pass.to_owned(),
                    reasoning_trace: None,
                    commit_hash: None,
                    prompt_hash: None,
                    prior_sir: PriorSir::Unrecorded,
                },
                None,
            )
            .expect("persist payload");
    };
    persist(&demo_sir(), "scan");
    let built_against = current_sir_identity(&store, id).expect("identity");
    let key = format!("{id}|prompt-retry|build-1");
    let origins = HashMap::from([(
        key.clone(),
        BatchRequestOrigin {
            prior_sir: built_against,
            source_hash: symbol.content_hash.clone(),
        },
    )]);
    let batch_sir = SirAnnotation {
        intent: "Triage result".to_owned(),
        ..demo_sir()
    };
    let provider = StubBatchProvider {
        key: key.clone(),
        text: serde_json::to_string(&batch_sir).expect("serialize sir"),
        reasoning_trace: Some("triage reasoning".to_owned()),
    };
    let prepare = |origins: &HashMap<String, BatchRequestOrigin>| {
        prepare_symbol(
            &pipeline,
            &store,
            &triage_pass_config(),
            "ignored",
            &provider,
            "gemini",
            &HashMap::new(),
            origins,
        )
        .expect("prepare symbol")
    };

    // The first attempt persists the SIR (and, in the real flow, then fails in the
    // embedding or fingerprint phase, so the sidecars are kept for a retry).
    let first = prepare(&origins).expect("first attempt applies the result");
    assert!(!first.resumed);
    let written = current_sir_identity(&store, id)
        .expect("identity")
        .expect("sir written");
    assert_eq!(written.sir_hash, aether_sir::sir_hash(&batch_sir));

    // The retry finds the store holding exactly this result's SIR: it is resumed
    // for the downstream work, not dropped as superseded, and the leaf is left as
    // it is (same history version and write generation).
    let retry = prepare(&origins).expect("retry resumes the result");
    assert!(retry.resumed, "the retry must resume, not rewrite");
    assert_eq!(retry.sir_hash, written.sir_hash);
    assert_eq!(retry.canonical_json, first.canonical_json);
    assert_eq!(
        current_sir_identity(&store, id).expect("identity"),
        Some(written.clone()),
        "resuming must not rewrite the leaf"
    );
    let meta = store
        .get_sir_meta(id)
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-retry"));
    assert_eq!(meta.generation_pass, "triage");

    // The row itself records which request wrote it, in the leaf's transaction.
    let meta = store
        .get_sir_meta(id)
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-retry"));
    assert_eq!(meta.generation_pass, "triage");

    // An independent write of the very same content (an injection, say) is not
    // this result's own: it carries no batch provenance, so the result is superseded
    // and the injection keeps its provenance.
    persist(&batch_sir, "injected");
    let injected = current_sir_identity(&store, id).expect("identity");
    assert_ne!(injected, Some(written.clone()));
    assert!(
        prepare(&origins).is_none(),
        "equal content does not identify the writer"
    );
    let meta = store
        .get_sir_meta(id)
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.generation_pass, "injected");
    assert_eq!(meta.prompt_hash, None);
    assert_eq!(
        current_sir_identity(&store, id).expect("identity"),
        injected
    );

    // A different SIR written since is superseded as before.
    persist(
        &SirAnnotation {
            intent: "Reviewed by hand".to_owned(),
            ..demo_sir()
        },
        "injected",
    );
    let reviewed = current_sir_identity(&store, id).expect("identity");
    assert!(
        prepare(&origins).is_none(),
        "a replaced SIR supersedes the result"
    );
    assert_eq!(
        current_sir_identity(&store, id).expect("identity"),
        reviewed
    );
}

struct FixedEmbeddingProvider;

#[async_trait]
impl aether_infer::EmbeddingProvider for FixedEmbeddingProvider {
    async fn embed_text(&self, _text: &str) -> Result<Vec<f32>, aether_infer::InferError> {
        Ok(vec![1.0, 0.0])
    }

    async fn embed_text_with_purpose(
        &self,
        _text: &str,
        _purpose: EmbeddingPurpose,
    ) -> Result<Vec<f32>, aether_infer::InferError> {
        Ok(vec![1.0, 0.0])
    }

    async fn embed_texts_with_purpose(
        &self,
        texts: &[&str],
        _purpose: EmbeddingPurpose,
    ) -> Result<Vec<Vec<f32>>, aether_infer::InferError> {
        Ok(vec![vec![1.0, 0.0]; texts.len()])
    }
}

#[test]
fn a_resumed_result_writes_its_fingerprint_row_once_against_its_true_predecessor() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);
    let config = aether_config::load_workspace_config(workspace).expect("load config");

    let store = SqliteStore::open(workspace).expect("open store");
    let pipeline = SirPipeline::new_embeddings_only_with(
        workspace.to_path_buf(),
        std::sync::Arc::new(FixedEmbeddingProvider),
        "test_embedding".to_owned(),
        "test-model".to_owned(),
        None,
    )
    .map(|pipeline| pipeline.with_skip_surreal_sync(true))
    .expect("build embeddings-only pipeline");
    let source = "fn fp() {}\n";
    let (symbol, record) = parsed_symbol(workspace, "src/lib.rs", source);
    store.upsert_symbol(record).expect("upsert symbol");
    let id = symbol.id.as_str();

    // The scan SIR the batch was built against, and the fingerprint row of the
    // prompt that produced it.
    pipeline
        .persist_sir_payload_into_sqlite(
            &store,
            &UpsertSirIntentPayload {
                symbol: symbol.clone(),
                sir: demo_sir(),
                provider_name: "gemini".to_owned(),
                model_name: "scan-model".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                commit_hash: None,
                prompt_hash: None,
                prior_sir: PriorSir::Unrecorded,
            },
            None,
        )
        .expect("persist scan sir");
    write_fingerprint_row(
        &store,
        id,
        "prompt-scan",
        None,
        "batch_scan",
        "scan-model",
        "scan",
        None,
        None,
    )
    .expect("scan fingerprint");
    let built_against = current_sir_identity(&store, id).expect("identity");
    let key = format!("{id}|prompt-fp|build-1");
    let origins = HashMap::from([(
        key.clone(),
        BatchRequestOrigin {
            prior_sir: built_against,
            source_hash: symbol.content_hash.clone(),
        },
    )]);
    let batch_sir = SirAnnotation {
        intent: "Triage result".to_owned(),
        ..demo_sir()
    };
    let provider = StubBatchProvider {
        key,
        text: serde_json::to_string(&batch_sir).expect("serialize sir"),
        reasoning_trace: None,
    };
    let chunk = |summary: &mut IngestSummary| {
        let mut buffer = Vec::new();
        process_chunk(
            &pipeline,
            &store,
            &triage_pass_config(),
            &config,
            &provider,
            "gemini",
            &HashMap::new(),
            &origins,
            &["ignored".to_owned()],
            summary,
            &mut buffer,
        )
        .expect("process chunk");
    };

    // The first attempt persists the SIR and its fingerprint row, then (say) fails
    // in the vector flush. The retry resumes: no second row.
    let mut summary = IngestSummary::default();
    chunk(&mut summary);
    assert_eq!(
        (summary.processed, summary.resumed, summary.fingerprint_rows),
        (1, 0, 1)
    );
    let mut summary = IngestSummary::default();
    chunk(&mut summary);
    assert_eq!(
        (summary.processed, summary.resumed, summary.fingerprint_rows),
        (1, 1, 0)
    );
    let history = store.list_sir_fingerprint_history(id).expect("history");
    assert_eq!(history.len(), 2, "scan row plus one batch row: {history:?}");
    let batch_row = &history[1];
    assert_eq!(batch_row.prompt_hash, "prompt-fp");
    assert_eq!(
        batch_row.prompt_hash_previous.as_deref(),
        Some("prompt-scan")
    );

    // An older ingest of the very same prompt left a row of its own (no write
    // generation, or another write's); a later ingest of that prompt that failed
    // before its row is still a new event: the retry writes it rather than taking
    // the old row for this write's, however close in time the two are.
    let history_before = store
        .list_sir_fingerprint_history(id)
        .expect("history")
        .len();
    write_fingerprint_row(
        &store,
        id,
        "prompt-fp",
        Some("prompt-scan"),
        "batch_triage",
        "triage-model",
        "triage",
        None,
        None,
    )
    .expect("older same-prompt row");
    prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider,
        "gemini",
        &HashMap::new(),
        &HashMap::from([(
            format!("{id}|prompt-fp|build-1"),
            BatchRequestOrigin {
                prior_sir: current_sir_identity(&store, id).expect("identity"),
                source_hash: symbol.content_hash.clone(),
            },
        )]),
    )
    .expect("prepare symbol")
    .expect("applied");
    let mut summary = IngestSummary::default();
    chunk(&mut summary);
    assert_eq!((summary.resumed, summary.fingerprint_rows), (1, 1));
    assert_eq!(
        store
            .list_sir_fingerprint_history(id)
            .expect("history")
            .len(),
        history_before + 2,
        "the older same-prompt row does not stand in for this write's"
    );

    // An attempt that persisted the SIR but failed before its fingerprint row leaves
    // no row; the resumed retry writes it against the last prompt the symbol was
    // fingerprinted for, not against this result's own write.
    let (symbol2, record2) = parsed_symbol(workspace, "src/fp2.rs", "fn fp2() {}\n");
    store.upsert_symbol(record2).expect("upsert symbol");
    let id2 = symbol2.id.as_str();
    pipeline
        .persist_sir_payload_into_sqlite(
            &store,
            &UpsertSirIntentPayload {
                symbol: symbol2.clone(),
                sir: demo_sir(),
                provider_name: "gemini".to_owned(),
                model_name: "scan-model".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                commit_hash: None,
                prompt_hash: None,
                prior_sir: PriorSir::Unrecorded,
            },
            None,
        )
        .expect("persist scan sir");
    write_fingerprint_row(
        &store,
        id2,
        "prompt-scan-2",
        None,
        "batch_scan",
        "scan-model",
        "scan",
        None,
        None,
    )
    .expect("scan fingerprint");
    let key2 = format!("{id2}|prompt-fp2|build-1");
    let origins2 = HashMap::from([(
        key2.clone(),
        BatchRequestOrigin {
            prior_sir: current_sir_identity(&store, id2).expect("identity"),
            source_hash: symbol2.content_hash.clone(),
        },
    )]);
    let provider2 = StubBatchProvider {
        key: key2,
        text: serde_json::to_string(&batch_sir).expect("serialize sir"),
        reasoning_trace: None,
    };
    // Phase 1 alone: the SIR lands, no fingerprint row yet.
    prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider2,
        "gemini",
        &HashMap::new(),
        &origins2,
    )
    .expect("prepare symbol")
    .expect("applied");
    let mut summary = IngestSummary::default();
    let mut buffer = Vec::new();
    process_chunk(
        &pipeline,
        &store,
        &triage_pass_config(),
        &config,
        &provider2,
        "gemini",
        &HashMap::new(),
        &origins2,
        &["ignored".to_owned()],
        &mut summary,
        &mut buffer,
    )
    .expect("process chunk");
    assert_eq!((summary.resumed, summary.fingerprint_rows), (1, 1));
    let history = store.list_sir_fingerprint_history(id2).expect("history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].prompt_hash, "prompt-fp2");
    assert_eq!(
        history[1].prompt_hash_previous.as_deref(),
        Some("prompt-scan-2"),
        "the predecessor is the last fingerprinted prompt, not this result's own"
    );
}

#[test]
fn prepare_symbol_promotes_metadata_when_sir_hash_is_unchanged() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path();
    write_embeddings_only_config(workspace);

    let store = SqliteStore::open(workspace).expect("open store");
    let record = demo_symbol_record("sym-batch", "demo::run");
    store.upsert_symbol(record.clone()).expect("upsert symbol");

    let pipeline = SirPipeline::new_embeddings_only(workspace.to_path_buf())
        .map(|pipeline| pipeline.with_skip_surreal_sync(true))
        .expect("build embeddings-only pipeline");
    let sir = demo_sir();
    let symbol = symbol_from_record(&record).expect("build symbol");

    pipeline
        .persist_sir_payload_into_sqlite(
            &store,
            &UpsertSirIntentPayload {
                symbol: symbol.clone(),
                sir: sir.clone(),
                provider_name: "gemini".to_owned(),
                model_name: "scan-model".to_owned(),
                generation_pass: "scan".to_owned(),
                reasoning_trace: None,
                commit_hash: None,
                prompt_hash: None,
                prior_sir: PriorSir::Unrecorded,
            },
            None,
        )
        .expect("persist scan payload");

    let provider = StubBatchProvider {
        key: "sym-batch|prompt-123".to_owned(),
        text: serde_json::to_string(&sir).expect("serialize sir"),
        reasoning_trace: Some("triage reasoning".to_owned()),
    };
    prepare_symbol(
        &pipeline,
        &store,
        &triage_pass_config(),
        "ignored",
        &provider,
        "gemini",
        &HashMap::new(),
        &HashMap::new(),
    )
    .expect("prepare symbol")
    .expect("result applied");

    let meta = store
        .get_sir_meta("sym-batch")
        .expect("load sir meta")
        .expect("sir meta exists");
    assert_eq!(meta.sir_version, 1);
    assert_eq!(meta.generation_pass, "triage");
    assert_eq!(meta.prompt_hash.as_deref(), Some("prompt-123"));
    assert_eq!(meta.reasoning_trace.as_deref(), Some("triage reasoning"));
}
