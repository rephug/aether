# Codex Prompt: Turbo Index — Bulk Concurrent Scan Pass

## Context

Read `docs/roadmap/turbo_index_session_context.md` for verified code references and architectural context.

The scan pass in `run_full_index_once_inner()` (crates/aetherd/src/indexer.rs) processes files **serially**, making one API round-trip per file. For ~5,000 symbols across ~500 files, this takes 30-90 minutes. The triage/deep passes already use a bulk concurrent pattern (`process_quality_batch`) that submits ALL symbols at once. The scan pass needs the same treatment.

Additionally, `commit_successful_generation` calls `refresh_embedding_if_needed` per symbol (one HTTP call each). The batch ingest pipeline already solved this with `batch_embed_texts` (chunks of 100) and `flush_embedding_batch` (chunks of 50).

**Goal:** Process 5,000 symbols in ~2-3 minutes by saturating the API's real-time rate limit.

## Preflight

```bash
# Ensure clean working tree
git status --porcelain
# Should be empty. If not, stash or commit first.

git pull --ff-only

# Create worktree
git worktree add -B feature/turbo-index /home/rephu/feature/turbo-index

cd /home/rephu/feature/turbo-index

export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Mandatory Source Inspection

Before writing any code, inspect these files and answer the questions:

1. Read `crates/aetherd/src/indexer.rs` lines 573-700. Identify:
   - The exact per-file scan loop (the `for (file_path, symbols) in symbols_by_file` block)
   - What `process_event_with_priority_and_pass` does beyond inference (symbol upserts, edge replacement, file rollups)
   - Whether `structural.process_event` already handles symbol upserts and edges before the scan loop

2. Read `crates/aetherd/src/sir_pipeline/mod.rs` method `process_quality_batch` (around line 485). Identify:
   - How it collects all jobs into one Vec
   - How it calls `generate_sir_jobs` once for all symbols
   - How it handles file rollups after inference
   - Whether it calls `commit_successful_generation` (which includes per-symbol embedding)

3. Read `crates/aetherd/src/sir_pipeline/mod.rs` method `commit_successful_generation` (around line 897). Identify:
   - The exact call to `refresh_embedding_if_needed` and where it sits in the commit flow
   - What state changes happen before and after the embedding call
   - Whether we can skip the embedding call and still leave the system in a consistent state (check WriteIntentStatus progression)

4. Read `crates/aetherd/src/batch/ingest.rs` function `process_chunk` (around line 140). Identify:
   - How it batches embedding API calls via `batch_embed_texts`
   - How it buffers and flushes to LanceDB via `flush_embedding_batch`
   - The `EmbeddingInput` struct fields
   - The constants `EMBED_BATCH_SIZE` (100) and `INGEST_VECTOR_BATCH_SIZE` (50)

5. Read `crates/aether-config/src/inference.rs` and `crates/aether-config/src/constants.rs`. Identify:
   - The `concurrency` field on `InferenceConfig`
   - `DEFAULT_SIR_CONCURRENCY` and `GEMINI_DEFAULT_CONCURRENCY` values
   - How `normalize_provider_concurrency` auto-bumps for Gemini

6. Read `crates/aetherd/src/cli.rs`. Identify:
   - The `index_once` flag and associated flags (`full`, `force`, `deep`, `reembed`)
   - Where to add a new `--turbo-concurrency` argument

## Implementation

### Step 1: Add `--turbo-concurrency` CLI flag

In `crates/aetherd/src/cli.rs`, add a new argument near the other `--index-once` flags:

```rust
#[arg(
    long,
    requires = "index_once",
    help = "Override inference concurrency for bulk scan. Set high (64-128) to saturate API rate limits for fast cold-start indexing"
)]
pub turbo_concurrency: Option<usize>,
```

This is purely a concurrency override — no new command, no new mode. The existing `--index-once --full` path gets faster when this flag is set.

### Step 2: Add `process_bulk_scan` method to `SirPipeline`

In `crates/aetherd/src/sir_pipeline/mod.rs`, add a new public method modeled on `process_quality_batch` but adapted for scan:

```rust
pub fn process_bulk_scan(
    &self,
    store: &SqliteStore,
    symbols: Vec<Symbol>,
    priority_scores: &HashMap<String, f64>,
    force: bool,
    generation_pass: &str,
    print_sir: bool,
    out: &mut dyn Write,
) -> Result<ProcessEventStats>
```

This method must:

**a) Build all SirJobs in one flat Vec:**
- Iterate over all symbols
- For each, check `should_skip_sir_generation` if not force (same as current per-file path)
- Call `build_job(workspace_root, symbol, priority_score, None)` — no enrichment, no custom prompt (scan pass)
- Track `touched_files: BTreeMap<String, Language>` for rollups
- Log progress: "Building SIR jobs: {built}/{total} ({skipped} skipped)"

**b) Submit all jobs to `generate_sir_jobs` in one call:**
- Use `self.sir_concurrency` as the concurrency parameter
- Log: "Submitting {count} scan jobs with concurrency {self.sir_concurrency}"

**c) Commit SIR results to SQLite WITHOUT per-symbol embedding:**
- For each `SirGenerationOutcome::Success`, do everything `commit_successful_generation` does EXCEPT the `refresh_embedding_if_needed` call and the VectorDone intent status update
- Specifically: `record_generation_quality`, `prepare_sir_for_persistence`, create write intent, `record_sir_version_if_changed`, `write_sir_blob`, `upsert_sir_meta`, update intent to `SqliteDone`
- Collect the canonical JSON + symbol_id + sir_hash for each success into a Vec for batch embedding later
- For failures, call `handle_failed_generation` as normal

**d) Batch-embed all results:**
- Collect all successful results into `Vec<EmbeddingInput>` using `self.embedding_identity()`
- Process in chunks of `EMBED_BATCH_SIZE` (100):
  - Call `self.batch_embed_texts(&texts, EmbeddingPurpose::Document)`
  - Build records via `SirPipeline::build_embedding_records`
  - Accumulate into embedding buffer
  - Flush buffer when it reaches `INGEST_VECTOR_BATCH_SIZE` (50) via `self.flush_embedding_batch`
- Flush any remaining buffer at the end
- After all embeddings written, update all intents to `VectorDone`
- Log: "Embedded {count} symbols in {chunks} batch calls"

**e) File rollups:**
- Iterate over `touched_files` and call `upsert_file_rollup` for each (same as `process_quality_batch`)

**f) Return stats:**
- Return `ProcessEventStats` with success_count + failure_count

### Step 3: Wire `process_bulk_scan` into the scan pass in `indexer.rs`

In `run_full_index_once_inner` (indexer.rs), replace the per-file scan loop with a call to `process_bulk_scan`.

Currently (simplified):
```rust
for (file_path, symbols) in symbols_by_file {
    let event = SymbolChangeEvent { ... };
    sir_pipeline.process_event_with_priority_and_pass(&store, &event, ...)?;
}
```

Replace with:
```rust
// Flatten all candidate symbols into one Vec
let mut all_scan_symbols = Vec::new();
for (_file_path, symbols) in symbols_by_file {
    all_scan_symbols.extend(symbols);
}

let scan_stats = sir_pipeline.process_bulk_scan(
    &store,
    all_scan_symbols,
    &priority_scores,
    config.force,
    SIR_GENERATION_PASS_SCAN,
    config.print_sir,
    &mut stdout,
)?;
tracing::info!(
    successes = scan_stats.success_count,
    failures = scan_stats.failure_count,
    "Bulk scan complete"
);
```

**IMPORTANT:** The structural indexing loop (`structural.process_event`) and symbol reconciliation that run BEFORE this loop must remain unchanged. They handle edge replacement and symbol upserts. The scan pass only needs to do inference + write SIR + embed.

Verify in source inspection: does `process_event_with_priority_and_pass` call `upsert_changed_symbols` redundantly with `structural.process_event`? If so, the bulk path can skip that upsert.

### Step 4: Thread `turbo_concurrency` through to `SirPipeline`

In `crates/aetherd/src/indexer.rs`, find where `SirPipeline::new` is called for the full index path (in `initialize_full_indexer` or similar). If `turbo_concurrency` is set on the CLI args, pass it as an override to the concurrency parameter.

The cleanest approach: add a method `pub fn with_concurrency_override(mut self, concurrency: usize) -> Self` to `SirPipeline` that sets `self.sir_concurrency` directly. Call it after construction if `turbo_concurrency` is set.

Thread the CLI arg through `IndexerConfig`:
- Add `pub turbo_concurrency: Option<usize>` to `IndexerConfig` in indexer.rs
- Set it from CLI args where IndexerConfig is constructed
- Apply it after `SirPipeline::new`:
  ```rust
  let sir_pipeline = if let Some(tc) = config.turbo_concurrency {
      sir_pipeline.with_concurrency_override(tc)
  } else {
      sir_pipeline
  };
  ```

### Step 5: Import required types

The bulk scan method needs access to types from batch ingest. Check if these are already pub(crate) accessible:
- `EmbeddingInput` — currently in `sir_pipeline/mod.rs`, should be accessible
- `EMBED_BATCH_SIZE`, `INGEST_VECTOR_BATCH_SIZE` — currently in `batch/ingest.rs`. Either re-export as pub(crate) constants or define new constants in `sir_pipeline/mod.rs` (they're just 100 and 50)

Prefer duplicating the two simple constants in `sir_pipeline/mod.rs` over creating cross-module dependencies.

## Scope Guard

**Files modified:**
- `crates/aetherd/src/cli.rs` — add `--turbo-concurrency` flag
- `crates/aetherd/src/sir_pipeline/mod.rs` — add `process_bulk_scan` method, add `with_concurrency_override` method
- `crates/aetherd/src/indexer.rs` — replace per-file scan loop with `process_bulk_scan` call, thread turbo_concurrency through IndexerConfig

**Files NOT modified:**
- No changes to batch pipeline (`batch/*.rs`)
- No changes to config crate (`aether-config`)
- No changes to inference providers (`aether-infer`)
- No schema changes
- No new crates

## Validation

```bash
# Format check
cargo fmt --all --check

# Clippy
cargo clippy -p aetherd --features dashboard -- -D warnings

# Per-crate tests (Do NOT run cargo test --workspace — OOM risk)
cargo test -p aetherd
cargo test -p aether-config

# Quick smoke test: dry run to verify job counting
# (run from a workspace with .aether/ initialized)
# aetherd --index-once --full --dry-run --workspace /path/to/repo
```

Do NOT run `cargo test --workspace` — it causes OOM on Codex and duplicates CI coverage.

## Commit

```
feat(indexer): bulk concurrent scan pass for fast cold-start indexing

Replace the serial per-file scan loop with a bulk concurrent submission
that sends all symbols to generate_sir_jobs in one JoinSet. Embeddings
are batched (100 per API call) and LanceDB writes are buffered (50 per
flush), matching the batch ingest pipeline pattern.

The --turbo-concurrency flag overrides inference concurrency for the
scan pass, allowing users to saturate API rate limits (e.g., 64-128
for Gemini's 4,000 RPM) for fast cold-start indexing.

Before: ~5,000 symbols processed in 30-90 minutes (serial per-file)
After: ~5,000 symbols processed in 2-3 minutes at concurrency 100
```

## Post-fix Cleanup

```bash
git push origin feature/turbo-index
```

Create PR via GitHub web UI with title and body from commit message above.

After merge:
```bash
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/turbo-index
git branch -D feature/turbo-index
```

## PR Title

`feat(indexer): bulk concurrent scan pass with batched embeddings for fast cold-start`

## PR Body

Replace the serial per-file scan loop in `run_full_index_once_inner` with a bulk
concurrent submission modeled on `process_quality_batch`. All candidate symbols
are collected into a flat `Vec<SirJob>` and submitted to `generate_sir_jobs` in
one call, with configurable concurrency via `--turbo-concurrency`.

Embedding writes follow the batch ingest pattern: `batch_embed_texts` in chunks
of 100, `flush_embedding_batch` in chunks of 50.

**Performance target:** 5,000 symbols in ~2-3 minutes at concurrency 100
(vs 30-90 minutes with serial per-file processing).

**Usage:**
```
aetherd --index-once --full --turbo-concurrency 100 --workspace .
```

Without `--turbo-concurrency`, the existing concurrency default applies (16 for
Gemini, 2 for other providers) — behavior is unchanged for existing users but the
bulk submission pattern still eliminates per-file serialization overhead.
