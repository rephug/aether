# Session Context: Turbo Index (Bulk Concurrent Scan)

## Problem

The scan pass in `run_full_index_once_inner()` (indexer.rs:573) iterates over files **serially**:

```
for (file_path, symbols) in symbols_by_file {
    sir_pipeline.process_event_with_priority_and_pass(store, &event, ...)?;
}
```

Each call to `process_event_with_priority_and_pass` dispatches inference through `generate_sir_jobs` which uses a Semaphore+JoinSet for concurrency — but since each event contains only one file's symbols (typically 1-20), the concurrency window is tiny. For a codebase with ~5,000 symbols across ~500 files, this means ~500 sequential round-trips to the API.

Additionally, `commit_successful_generation` (sir_pipeline/mod.rs:897) calls `refresh_embedding_if_needed` per symbol, which makes one embedding API call per symbol — another ~5,000 sequential HTTP calls.

Result: a full scan that should take ~2 minutes at 4,000 RPM takes 30-90 minutes.

**The triage and deep passes don't have this problem.** They use `process_quality_batch` (sir_pipeline/mod.rs:485) which collects ALL symbols into one flat Vec and submits them all at once to `generate_sir_jobs`. But the scan pass predates this pattern.

## Goal

Process 5,000 symbols in ~2-3 minutes instead of 30-90 minutes by:
1. Submitting all scan jobs in one `generate_sir_jobs` call with high concurrency
2. Batching embedding API calls (chunks of 100)
3. Batching LanceDB vector writes (chunks of 50)

## Existing Infrastructure (verified in source)

### Concurrent inference engine
- **`generate_sir_jobs`** (sir_pipeline/infer.rs:157): Takes `Vec<SirJob>`, `concurrency: usize`. Uses `Arc<Semaphore>` + `JoinSet` to run up to `concurrency` inference calls in parallel. Returns `Vec<SirGenerationOutcome>`. Already handles per-task failures gracefully.

### Bulk triage/deep pattern (the template to follow)
- **`process_quality_batch`** (sir_pipeline/mod.rs:485): Collects all candidate symbols, builds all SirJobs, calls `generate_sir_jobs` once with full list, then commits results. This is exactly what the scan pass should do.

### Job building
- **`build_job`** (sir_pipeline/infer.rs:48): Takes `(workspace_root, symbol, priority_score, max_chars)`, reads source file, extracts symbol text, builds `SirJob`. No enrichment context for scan (that's only for triage/deep).

### Batch embedding (from batch pipeline PRs #113-117)
- **`batch_embed_texts`** (sir_pipeline/mod.rs:1937): Calls `embedding_provider.embed_texts_with_purpose(texts, purpose)`. Processes up to 100 texts per API call (Gemini `batchEmbedContents` limit).
- **`build_embedding_records`** (sir_pipeline/mod.rs:1951): Builds `Vec<SymbolEmbeddingRecord>` from embedding vectors + metadata.
- **`flush_embedding_batch`** (sir_pipeline/mod.rs:1879): Batch-writes `Vec<SymbolEmbeddingRecord>` to LanceDB via `vector_store.upsert_embeddings_batch()`.
- **`EmbeddingInput`** struct (sir_pipeline/mod.rs): `{ symbol_id, sir_hash, canonical_json, provider, model }`.

### Per-symbol commit (current, not batched)
- **`commit_successful_generation`** (sir_pipeline/mod.rs:897): Per-symbol: write_intent → sir_version → sir_blob → sir_meta → update_intent_status → **refresh_embedding_if_needed** → update_intent_status. The embedding refresh is the bottleneck — it's per-symbol HTTP call.

### Concurrency defaults
- `DEFAULT_SIR_CONCURRENCY = 2` (aether-config/src/constants.rs:13)
- `GEMINI_DEFAULT_CONCURRENCY = 16` (aether-config/src/constants.rs)
- `normalize_provider_concurrency` (aether-config/src/normalize.rs:126): Auto-bumps Gemini from default 2 → 16

### Existing config fields
- `[inference].concurrency` (aether-config/src/inference.rs:58-59): Controls SirPipeline concurrency. Default 2, auto-bumped to 16 for Gemini.
- `sir_concurrency` field on `SirPipeline` struct: set from config during `SirPipeline::new()`.

## Key Code Paths

### Current scan pass (indexer.rs:637-690)
```rust
// Symbols grouped by file
let mut symbols_by_file = BTreeMap::<String, Vec<Symbol>>::new();
for symbol_id in candidate_symbol_ids {
    if let Some(symbol) = symbols_by_id.get(symbol_id.as_str()) {
        symbols_by_file.entry(symbol.file_path.clone()).or_default().push(symbol.clone());
    } else {
        unresolved += 1;
    }
}

// Serial per-file loop — THE BOTTLENECK
for (file_path, symbols) in symbols_by_file {
    let event = SymbolChangeEvent { file_path, language: symbols[0].language, ... };
    sir_pipeline.process_event_with_priority_and_pass(&store, &event, ...)?;
}
```

### process_quality_batch pattern (sir_pipeline/mod.rs:485-630)
```rust
// Collects ALL jobs into one Vec
let mut jobs = Vec::with_capacity(items.len());
for item in items {
    match build_job(&self.workspace_root, item.symbol, ...) {
        Ok(mut job) => { /* set custom_prompt for triage */ jobs.push(job); }
        Err(err) => { stats.failure_count += 1; }
    }
}

// One concurrent submission for ALL symbols
let results = self.runtime.block_on(generate_sir_jobs(
    self.provider.clone(), ..., jobs, self.sir_concurrency, self.inference_timeout_secs,
))?;

// Commit results (still per-symbol)
for result in results {
    match result {
        SirGenerationOutcome::Success(generated) => {
            self.commit_successful_generation(store, *generated, ...)?;
        }
        ...
    }
}

// File rollups
for (file_path, language) in touched_files {
    self.upsert_file_rollup(store, ...)?;
}
```

### Batch ingest embedding pattern (batch/ingest.rs:140-200)
```rust
// Phase 1: Parse + persist SIR, queue embeddings
for raw_line in lines {
    prep.embedding_slot = Some(embed_inputs.len());
    embed_inputs.push(EmbeddingInput { symbol_id, sir_hash, canonical_json, provider, model });
    prepared.push(prep);
}

// Phase 2: Batch-embed all at once
let texts: Vec<&str> = embed_inputs.iter().map(|i| i.canonical_json.as_str()).collect();
let embeddings = pipeline.batch_embed_texts(&texts, EmbeddingPurpose::Document)?;
let records = SirPipeline::build_embedding_records(&embed_inputs, embeddings);

// Phase 3: Buffer + flush to LanceDB in batches of 50
for record in records {
    embedding_buffer.push(record);
    if embedding_buffer.len() >= INGEST_VECTOR_BATCH_SIZE {
        pipeline.flush_embedding_batch(std::mem::take(embedding_buffer))?;
    }
}
```

## Struct/Type References

```rust
// sir_pipeline/infer.rs:20
pub(crate) struct SirJob {
    pub(crate) symbol: Symbol,
    pub(crate) symbol_text: String,
    pub(crate) context: SirContext,
    pub(crate) custom_prompt: Option<String>,  // None for scan, Some for triage/deep
    pub(crate) deep_mode: bool,                // false for scan
}

// sir_pipeline/mod.rs
pub(crate) struct EmbeddingInput {
    pub(crate) symbol_id: String,
    pub(crate) sir_hash: String,
    pub(crate) canonical_json: String,
    pub(crate) provider: String,
    pub(crate) model: String,
}

// sir_pipeline/infer.rs
pub(super) enum SirGenerationOutcome {
    Success(Box<GeneratedSir>),
    Failure(Box<FailedSirGeneration>),
}

pub(super) struct GeneratedSir {
    pub(super) symbol: Symbol,
    pub(super) sir: SirAnnotation,
    pub(super) provider_name: String,
    pub(super) model_name: String,
}
```

## Constants

```rust
// sir_pipeline/mod.rs
const INFERENCE_MAX_RETRIES: usize = 2;
const INFERENCE_ATTEMPT_TIMEOUT_SECS: u64 = 90;
const MAX_SYMBOL_TEXT_CHARS: usize = 10_000;

// batch/ingest.rs
const INGEST_VECTOR_BATCH_SIZE: usize = 50;
const EMBED_BATCH_SIZE: usize = 100;
```

## Rate Limits (from provider docs, not in code)

| Provider | Real-time RPM | Concurrency sweet spot |
|---|---|---|
| Gemini flash-lite | 4,000 | 64-100 |
| OpenAI gpt-5.4-mini | 10,000 | 100-200 |
| Anthropic Haiku | 4,000 | 64-100 |

No explicit rate limiting exists in the inference providers. The Semaphore is the only throttle. 429 responses trigger the existing retry logic in `generate_sir_with_retries`.
