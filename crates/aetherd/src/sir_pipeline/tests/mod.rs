use std::fs;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use aether_core::{
    EdgeKind, Language, Position, SourceRange, Symbol, SymbolEdge, SymbolKind, content_hash,
};
use aether_store::SemanticIndexStore;
use async_trait::async_trait;
use rusqlite::Connection;
use tempfile::tempdir;

mod embeddings;
mod persistence;
mod processing;

#[derive(Clone)]
struct CountingEmbeddingProvider {
    calls: Arc<AtomicUsize>,
    batch_calls: Arc<AtomicUsize>,
    batch_sizes: Arc<Mutex<Vec<usize>>>,
    purposes: Arc<Mutex<Vec<EmbeddingPurpose>>>,
}

#[async_trait]
impl EmbeddingProvider for CountingEmbeddingProvider {
    async fn embed_text(&self, _text: &str) -> std::result::Result<Vec<f32>, InferError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.purposes
            .lock()
            .expect("purposes mutex")
            .push(EmbeddingPurpose::Document);
        Ok(vec![1.0, 0.0])
    }

    async fn embed_text_with_purpose(
        &self,
        _text: &str,
        purpose: EmbeddingPurpose,
    ) -> std::result::Result<Vec<f32>, InferError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.purposes.lock().expect("purposes mutex").push(purpose);
        Ok(vec![1.0, 0.0])
    }

    async fn embed_texts_with_purpose(
        &self,
        texts: &[&str],
        purpose: EmbeddingPurpose,
    ) -> std::result::Result<Vec<Vec<f32>>, InferError> {
        self.batch_calls.fetch_add(1, Ordering::SeqCst);
        self.batch_sizes
            .lock()
            .expect("batch sizes mutex")
            .push(texts.len());
        self.purposes.lock().expect("purposes mutex").push(purpose);
        Ok(vec![vec![1.0, 0.0]; texts.len()])
    }
}

struct PanicInferenceProvider;

#[derive(Clone)]
struct FixedInferenceProvider {
    sir: SirAnnotation,
}

#[derive(Clone)]
struct CountingInferenceProvider {
    symbol_sir: SirAnnotation,
    prompt_sir: SirAnnotation,
    standard_calls: Arc<AtomicUsize>,
    prompt_calls: Arc<AtomicUsize>,
    file_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl InferenceProvider for PanicInferenceProvider {
    fn provider_name(&self) -> String {
        "panic".to_owned()
    }

    fn model_name(&self) -> String {
        "panic".to_owned()
    }

    async fn generate_sir(
        &self,
        _symbol_text: &str,
        _context: &SirContext,
    ) -> std::result::Result<SirAnnotation, InferError> {
        panic!("embeddings-only pass must not call inference providers");
    }
}

#[async_trait]
impl InferenceProvider for FixedInferenceProvider {
    fn provider_name(&self) -> String {
        "fixed".to_owned()
    }

    fn model_name(&self) -> String {
        "fixed-model".to_owned()
    }

    async fn generate_sir(
        &self,
        _symbol_text: &str,
        _context: &SirContext,
    ) -> std::result::Result<SirAnnotation, InferError> {
        Ok(self.sir.clone())
    }

    async fn generate_sir_from_prompt(
        &self,
        _prompt: &str,
        _context: &SirContext,
        _deep_mode: bool,
    ) -> std::result::Result<SirAnnotation, InferError> {
        Ok(self.sir.clone())
    }
}

#[async_trait]
impl InferenceProvider for CountingInferenceProvider {
    fn provider_name(&self) -> String {
        "counting".to_owned()
    }

    fn model_name(&self) -> String {
        "counting-model".to_owned()
    }

    async fn generate_sir(
        &self,
        _symbol_text: &str,
        context: &SirContext,
    ) -> std::result::Result<SirAnnotation, InferError> {
        if context.kind == "file" {
            self.file_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.prompt_sir.clone())
        } else {
            self.standard_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.symbol_sir.clone())
        }
    }

    async fn generate_sir_from_prompt(
        &self,
        _prompt: &str,
        context: &SirContext,
        _deep_mode: bool,
    ) -> std::result::Result<SirAnnotation, InferError> {
        if context.kind == "file" {
            self.file_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.prompt_sir.clone())
        } else {
            self.prompt_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.symbol_sir.clone())
        }
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

fn demo_symbol(symbol_id: &str, qualified_name: &str) -> SymbolRecord {
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

fn demo_symbol_record_with_kind(
    symbol_id: &str,
    qualified_name: &str,
    kind: &str,
    file_path: &str,
) -> SymbolRecord {
    SymbolRecord {
        id: symbol_id.to_owned(),
        file_path: file_path.to_owned(),
        language: "rust".to_owned(),
        kind: kind.to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{symbol_id}"),
        last_seen_at: 1_700_000_000,
    }
}

fn demo_type_symbol(
    symbol_id: &str,
    name: &str,
    qualified_name: &str,
    file_path: &str,
    kind: SymbolKind,
    source: &str,
) -> Symbol {
    Symbol {
        id: symbol_id.to_owned(),
        language: Language::Rust,
        file_path: file_path.to_owned(),
        kind,
        name: name.to_owned(),
        qualified_name: qualified_name.to_owned(),
        signature_fingerprint: format!("sig-{symbol_id}"),
        content_hash: content_hash(source),
        range: SourceRange {
            start: Position { line: 1, column: 1 },
            end: Position {
                line: source.lines().count().max(1),
                column: source
                    .lines()
                    .last()
                    .map(|line| line.len() + 1)
                    .unwrap_or(1),
            },
            start_byte: Some(0),
            end_byte: Some(source.len()),
        },
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

fn demo_rollup_sir() -> SirAnnotation {
    SirAnnotation {
        intent: "File rollup summary".to_owned(),
        ..demo_sir()
    }
}

fn seed_sir(store: &SqliteStore, symbol_id: &str, sir: &SirAnnotation) -> String {
    let canonical = canonicalize_sir_json(sir);
    let hash = sir_hash(sir);
    store
        .write_sir_blob(symbol_id, &canonical)
        .expect("write sir blob");
    store
        .upsert_sir_meta(SirMetaRecord {
            id: symbol_id.to_owned(),
            sir_hash: hash.clone(),
            sir_version: 1,
            provider: "test".to_owned(),
            model: "test".to_owned(),
            generation_pass: SIR_GENERATION_PASS_SCAN.to_owned(),
            reasoning_trace: None,
            prompt_hash: None,
            staleness_score: None,
            updated_at: 1_700_000_100,
            sir_status: SIR_STATUS_FRESH.to_owned(),
            last_error: None,
            last_attempt_at: 1_700_000_100,
        })
        .expect("upsert sir meta");
    hash
}

fn build_embeddings_only_pipeline(
    workspace: &Path,
    embedding_provider: Arc<dyn EmbeddingProvider>,
) -> SirPipeline {
    SirPipeline::new_with_provider_and_embeddings(
        workspace.to_path_buf(),
        1,
        Arc::new(PanicInferenceProvider),
        "panic",
        "panic",
        Some(embedding_provider),
        Some(("test_embedding".to_owned(), "test-model".to_owned())),
        None,
        None,
        None,
    )
    .expect("build pipeline")
}

fn build_write_pipeline(workspace: &Path, provider: Arc<dyn InferenceProvider>) -> SirPipeline {
    build_write_pipeline_with_embeddings(workspace, provider, None)
}

fn build_write_pipeline_with_embeddings(
    workspace: &Path,
    provider: Arc<dyn InferenceProvider>,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
) -> SirPipeline {
    let embedding_identity = embedding_provider
        .as_ref()
        .map(|_| ("test_embedding".to_owned(), "test-model".to_owned()));
    SirPipeline::new_with_provider_and_embeddings(
        workspace.to_path_buf(),
        1,
        provider,
        "test_provider",
        "test_model",
        embedding_provider,
        embedding_identity,
        None,
        None,
        None,
    )
    .expect("build pipeline")
}

fn make_quality_batch_items(symbols: &[Symbol]) -> Vec<QualityBatchItem> {
    symbols
        .iter()
        .cloned()
        .map(|symbol| QualityBatchItem {
            symbol,
            priority_score: 0.9,
            enrichment: SirEnrichmentContext {
                file_intent: None,
                neighbor_intents: Vec::new(),
                baseline_sir: None,
                priority_reason: "test batch".to_owned(),
                caller_contract_clauses: Vec::new(),
            },
            use_cot: false,
            baseline_sir_identity: None,
        })
        .collect()
}

fn upsert_existing_embedding(workspace: &Path, symbol_id: &str, sir_hash: &str) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let vector_store = runtime
        .block_on(open_vector_store(workspace))
        .expect("open vector store");
    runtime
        .block_on(vector_store.upsert_embedding(SymbolEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: sir_hash.to_owned(),
            provider: "test_embedding".to_owned(),
            model: "test-model".to_owned(),
            embedding: vec![1.0, 0.0],
            updated_at: 1_700_000_200,
        }))
        .expect("seed embedding");
}

fn install_graph_done_failure_trigger(workspace: &Path, symbol_id: &str) {
    let conn =
        Connection::open(workspace.join(".aether/meta.sqlite")).expect("open sqlite database");
    conn.execute_batch(
        format!(
            r#"
            CREATE TRIGGER fail_graph_done_for_test
            BEFORE UPDATE OF status ON write_intents
            WHEN NEW.status = 'graph_done' AND OLD.symbol_id = '{symbol_id}'
            BEGIN
                SELECT RAISE(FAIL, 'graph_done blocked for test');
            END;
            "#
        )
        .as_str(),
    )
    .expect("install graph_done failure trigger");
}

fn install_sqlite_done_failure_trigger(workspace: &Path, symbol_id: &str) {
    let conn =
        Connection::open(workspace.join(".aether/meta.sqlite")).expect("open sqlite database");
    conn.execute_batch(
        format!(
            r#"
            CREATE TRIGGER fail_sqlite_done_for_test
            BEFORE UPDATE OF status ON write_intents
            WHEN NEW.status = 'sqlite_done' AND OLD.symbol_id = '{symbol_id}'
            BEGIN
                SELECT RAISE(FAIL, 'sqlite_done blocked for test');
            END;
            "#
        )
        .as_str(),
    )
    .expect("install sqlite_done failure trigger");
}

fn count_table_rows(workspace: &Path, table: &str) -> i64 {
    let conn =
        Connection::open(workspace.join(".aether/meta.sqlite")).expect("open sqlite database");
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .expect("count rows")
}

fn counting_embedding_provider() -> (CountingEmbeddingProvider, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = CountingEmbeddingProvider {
        calls: Arc::clone(&calls),
        batch_calls: Arc::new(AtomicUsize::new(0)),
        batch_sizes: Arc::new(Mutex::new(Vec::new())),
        purposes: Arc::new(Mutex::new(Vec::new())),
    };
    (provider, calls)
}

/// `still_current` answers from a script (one entry per call, `true` once the script
/// runs out) and records how often it was asked.
fn scripted_check(script: Vec<bool>) -> (Arc<Mutex<Vec<bool>>>, Arc<AtomicUsize>) {
    (Arc::new(Mutex::new(script)), Arc::new(AtomicUsize::new(0)))
}

fn payload_for(symbol: &Symbol, sir: &SirAnnotation, pass: &str) -> UpsertSirIntentPayload {
    UpsertSirIntentPayload {
        symbol: symbol.clone(),
        sir: sir.clone(),
        provider_name: "test_provider".to_owned(),
        model_name: "test_model".to_owned(),
        generation_pass: pass.to_owned(),
        reasoning_trace: None,
        commit_hash: None,
        prior_sir: PriorSir::Unrecorded,
    }
}
