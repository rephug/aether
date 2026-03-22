# Session Context: Parallel File Rollups + Batched Graph Completion

## Problem

After turbo index PRs #121-#122, inference is fast (~2.5 min scan, ~1.75 min triage for 5,443 symbols). The new bottleneck is **post-inference processing** — specifically file rollups and graph stage completion.

Timing from cold-start run (concurrency 50, flash-lite, embeddings disabled):

| Phase | Start | End | Duration |
|---|---|---|---|
| Structural indexing | 03:20:12 | 03:22:24 | 2 min |
| Scan inference | 03:22:24 | 03:24:52 | **2.5 min** |
| Scan SIR persist | 03:24:52 | 03:25:36 | 44 sec |
| Scan graph + rollups | 03:25:36 | 03:34:16 | **8.7 min** ← bottleneck |
| Triage inference | 03:34:27 | 03:36:12 | **1.75 min** |
| Triage SIR persist | 03:36:12 | 03:36:58 | 46 sec |
| Triage graph + rollups | 03:36:58 | 03:51:20 | **14.4 min** ← bottleneck |

8.7 + 14.4 = **23 minutes** on file rollups + graph completion. That's 74% of the 31-minute total.

## Root Cause 1: Serial File Rollup API Calls

`upsert_file_rollup` (sir_pipeline/mod.rs:2584) is called once per touched file in a serial loop. For files with >5 symbols, it calls `aggregate_file_sir` (rollup.rs:17) which calls `summarize_file_intent` (rollup.rs:91) — an LLM API call via `generate_sir_with_retries`.

With ~380 touched files and perhaps ~200 having >5 symbols, that's ~200 serial API calls at ~1-3 seconds each = 6-10 minutes per pass. This runs twice (scan + triage) = 12-20 minutes.

The fix: submit all file rollup API calls concurrently using the same JoinSet+Semaphore pattern as `generate_sir_jobs`.

## Root Cause 2: Serial Graph Stage Completion

`complete_graph_stage_without_sync` (sir_pipeline/mod.rs:1505) iterates over every intent ID and does 2 individual SQLite writes per intent:
1. `store.update_intent_status(intent_id, GraphDone)` — 1 UPDATE
2. `store.mark_intent_complete(intent_id)` — 1 UPDATE

For 5,443 intents = 10,886 individual SQL statements, each acquiring the Mutex<Connection> and doing a WAL sync. This takes 2-4 minutes per pass.

The fix: add a `batch_complete_intents` method to SqliteStore that wraps all intent completions in a single transaction.

## File Rollup Architecture (rollup.rs)

```rust
pub(super) fn aggregate_file_sir(
    file_path: &str,
    language: Language,
    leaf_sirs: &[FileLeafSir],
    provider: Arc<dyn InferenceProvider>,
    runtime: &Runtime,
    inference_timeout_secs: u64,
) -> Result<FileSir>
```

- If ≤5 symbols: `concatenate_leaf_intents` (deterministic, no API call)
- If >5 symbols: `summarize_file_intent` → `generate_sir_with_retries` (API call)
- Fallback: if API call fails, uses concatenation

The API call is inside `runtime.block_on(generate_sir_with_retries(...))` — it uses the SirPipeline's tokio runtime.

## Current Serial Rollup Loop (bulk scan path, sir_pipeline/mod.rs:845-856)

```rust
for (file_path, language) in touched_files {
    self.upsert_file_rollup(
        store, file_path.as_str(), language,
        print_sir, out, commit_hash.as_deref(), generation_pass,
    )
    .with_context(|| format!("failed to upsert bulk-scan file rollup for {file_path}"))?;
}
```

Same pattern in `process_quality_batch` (sir_pipeline/mod.rs:663).

## `upsert_file_rollup` Internals (sir_pipeline/mod.rs:2584)

Per file:
1. `store.list_symbols_for_file(file_path)` — 1 SELECT
2. `store.read_sir_blob(symbol_id)` per symbol — N SELECTs
3. `aggregate_file_sir(...)` — potential API call
4. `store.record_sir_version_if_changed(...)` — 1 IMMEDIATE transaction
5. `store.write_sir_blob(...)` — 1 INSERT/UPDATE + optional file mirror
6. `store.upsert_sir_meta(...)` — 1 INSERT/UPSERT

Steps 1-3 are read-heavy + potential API call. Steps 4-6 are writes.

## Current Graph Completion (sir_pipeline/mod.rs:1505-1533)

```rust
fn complete_graph_stage_without_sync(
    &self, store: &SqliteStore, intents_ready_for_graph: &mut Vec<String>,
) {
    for intent_id in intents_ready_for_graph.drain(..) {
        store.update_intent_status(&intent_id, WriteIntentStatus::GraphDone)?;
        store.mark_intent_complete(&intent_id)?;
    }
}
```

Each `update_intent_status` and `mark_intent_complete` acquires the Mutex, executes one SQL UPDATE, releases.

## SqliteStore Connection Model (aether-store/src/lib.rs:486)

```rust
pub struct SqliteStore {
    conn: Mutex<Connection>,
    // ...
}
```

Single connection behind a Mutex. WAL mode enabled. No existing batch/transaction helpers beyond what individual methods do internally.

`record_sir_version_if_changed` (sir_history.rs:226) internally creates `Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)`. This means you cannot naively wrap an outer transaction around calls to this method — SQLite doesn't support nested IMMEDIATE transactions.

## Key Types

```rust
// sir_pipeline/mod.rs
struct PersistedSuccessfulGeneration {
    intent_id: String,
    symbol_id: String,
    file_path: String,
    sir_hash: String,
    canonical_json: String,
    provider_name: String,
    embedding_needed: Option<EmbeddingNeeded>,
}

// rollup.rs
struct FileLeafSir {
    qualified_name: String,
    sir: SirAnnotation,
}
```

## Available Infrastructure

- `generate_sir_jobs` (sir_pipeline/infer.rs:157) — JoinSet+Semaphore concurrent inference pattern
- `generate_sir_with_retries` (sir_pipeline/infer.rs:354) — single-symbol inference with retry
- `self.runtime` on SirPipeline — tokio Runtime for async calls
- `self.sir_concurrency` — semaphore size for inference calls
- `self.provider.clone()` — Arc<dyn InferenceProvider>
