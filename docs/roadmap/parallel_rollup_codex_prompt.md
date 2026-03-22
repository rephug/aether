# Codex Prompt: Parallel File Rollups + Batched Graph Completion

## Context

Read `docs/roadmap/parallel_rollup_session_context.md` for verified code references and timing data.

After turbo index PRs (#121-#122), inference runs in ~2.5 min for 5,443 symbols. But the post-inference file rollup + graph completion phase takes **8-14 minutes per pass** — 74% of total runtime. Two causes:

1. **Serial file rollup API calls**: `upsert_file_rollup` is called in a serial loop over ~380 files. For files with >5 symbols, it makes an LLM API call (`summarize_file_intent` via `generate_sir_with_retries`). ~200 serial API calls at 1-3s each = 6-10 min per pass.

2. **Serial graph completion writes**: `complete_graph_stage_without_sync` does 2 individual SQLite writes per intent (10,886 ops for 5,443 symbols). Each acquires the Mutex separately.

**Goal:** Reduce post-inference processing from 8-14 minutes to under 1 minute per pass.

## Preflight

```bash
git status --porcelain
git pull --ff-only

git worktree add -B feature/parallel-rollup /home/rephu/feature/parallel-rollup
cd /home/rephu/feature/parallel-rollup

export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Mandatory Source Inspection

Before writing any code, inspect these files and answer the questions:

1. Read `crates/aetherd/src/sir_pipeline/rollup.rs` entirely. Identify:
   - The `aggregate_file_sir` function signature and how it decides between `concatenate_leaf_intents` (≤5 symbols) and `summarize_file_intent` (>5 symbols)
   - How `summarize_file_intent` calls `generate_sir_with_retries` through `runtime.block_on`
   - What `aggregate_file_sir` returns (`FileSir`) and what data it needs as input

2. Read `crates/aetherd/src/sir_pipeline/mod.rs` method `upsert_file_rollup` (around line 2584). Identify:
   - The serial loop in `process_bulk_scan` (around line 845) and `process_quality_batch` (around line 663) that calls `upsert_file_rollup` per file
   - What `upsert_file_rollup` does before the API call (reads from store) and after (writes to store)
   - Whether the store reads (list_symbols_for_file, read_sir_blob) can be done ahead of time

3. Read `crates/aetherd/src/sir_pipeline/mod.rs` method `complete_graph_stage_without_sync` (around line 1505). Identify:
   - The per-intent loop calling `update_intent_status` and `mark_intent_complete`
   - Whether the intent IDs come from `intents_by_file` (a BTreeMap<String, Vec<String>>)

4. Read `crates/aether-store/src/write_intents.rs` methods `update_intent_status` (line 151) and `mark_intent_complete` (line 185). Identify:
   - Each acquires `self.conn.lock().unwrap()` separately for one SQL statement
   - The SQL statements are simple UPDATE with WHERE intent_id = ?

5. Read `crates/aether-store/src/lib.rs` around line 486. Identify:
   - `SqliteStore { conn: Mutex<Connection> }` — single connection
   - WAL mode is enabled (line 522)
   - Whether any transaction/batch helpers exist (there should be none)

## Implementation

### Step 1: Add `batch_complete_intents` to SqliteStore

In `crates/aether-store/src/write_intents.rs`, add a new method:

```rust
/// Complete a batch of write intents in a single transaction.
/// Each intent is advanced from its current status → GraphDone → Complete.
pub fn batch_complete_intents(&self, intent_ids: &[String]) -> Result<BatchCompleteResult, StoreError>
```

Where `BatchCompleteResult` is a simple struct: `{ completed: usize, failed: usize }`.

Implementation:
- Acquire the connection lock once
- Begin a DEFERRED transaction
- For each intent_id: execute UPDATE to GraphDone, then UPDATE to Complete (same SQL as existing methods)
- On per-intent error: log warning, increment failed count, continue (don't abort batch)
- Commit transaction
- Return counts

This replaces the per-intent loop in `complete_graph_stage_without_sync` for bulk paths.

### Step 2: Add `batch_complete_intents_without_sync` to SirPipeline

In `crates/aetherd/src/sir_pipeline/mod.rs`, add a new method alongside `complete_graph_stage_without_sync`:

```rust
fn batch_complete_graph_stage_without_sync(
    &self,
    store: &SqliteStore,
    intents_by_file: BTreeMap<String, Vec<String>>,
) -> usize
```

This flattens all intent IDs from `intents_by_file` into one `Vec<String>` and calls `store.batch_complete_intents`. Returns total completed count. Log a summary.

### Step 3: Parallelize file rollup API calls

In `crates/aetherd/src/sir_pipeline/mod.rs`, add a new method:

```rust
fn bulk_upsert_file_rollups(
    &self,
    store: &SqliteStore,
    touched_files: BTreeMap<String, Language>,
    print_sir: bool,
    out: &mut dyn Write,
    commit_hash: Option<&str>,
    generation_pass: &str,
) -> Result<()>
```

This method:

**Phase 1 — Prepare rollup inputs (serial, read-only):**
For each (file_path, language) in touched_files:
- Call `store.list_symbols_for_file(file_path)` — collect symbol list
- For each symbol, call `store.read_sir_blob(symbol_id)` — collect SIR text
- Parse into `Vec<FileLeafSir>`
- Package as a `RollupJob { file_path, language, leaf_sirs }`
- Separate into two lists: `needs_api` (>5 symbols) and `local_only` (≤5 symbols)

**Phase 2 — Process local-only rollups immediately (no API call):**
For each `local_only` job, call `concatenate_leaf_intents` and build the `FileSir` directly. Then persist (record_sir_version_if_changed, write_sir_blob, upsert_sir_meta).

**Phase 3 — Submit API rollups concurrently:**
For the `needs_api` jobs, use the same JoinSet+Semaphore pattern:
- Create `Semaphore::new(self.sir_concurrency)`
- For each job, spawn a task that calls `summarize_file_intent` (or the equivalent) — note this function currently takes a Runtime and does block_on internally. For the concurrent path, make it async: call `generate_sir_with_retries` directly (it's already async).
- Collect results. On failure, fall back to `concatenate_leaf_intents` (same as current behavior in `aggregate_file_sir`).

**Phase 4 — Persist all API rollup results (serial writes):**
For each completed rollup result, persist to SQLite (record_sir_version_if_changed, write_sir_blob, upsert_sir_meta).

The key change: the inference API calls (Phase 3) run concurrently at full concurrency. The SQLite writes (Phases 2 + 4) remain serial but are fast (~380 writes total).

### Step 4: Make `summarize_file_intent` async-compatible

In `crates/aetherd/src/sir_pipeline/rollup.rs`, the `summarize_file_intent` function currently calls `runtime.block_on(generate_sir_with_retries(...))`. For the concurrent path, we need an async version.

Options (choose one during source inspection):
a. Extract the async body into a new `async fn summarize_file_intent_async(...)` that calls `generate_sir_with_retries` directly without `block_on`. The JoinSet tasks call this.
b. Keep `summarize_file_intent` as-is and have the JoinSet tasks call `generate_sir_with_retries` directly with the constructed prompt.

Option (b) is simpler — the prompt construction is ~10 lines that can be inlined or factored into a `build_file_rollup_prompt` helper.

### Step 5: Replace serial rollup loops in both bulk paths

In `process_bulk_scan` (around line 845), replace:
```rust
for (file_path, language) in touched_files {
    self.upsert_file_rollup(store, ...)?;
}
```
With:
```rust
self.bulk_upsert_file_rollups(store, touched_files, print_sir, out, commit_hash.as_deref(), generation_pass)?;
```

In `process_quality_batch` (around line 663), apply the same replacement.

### Step 6: Replace serial graph completion in both bulk paths

In `process_bulk_scan` (around line 842), replace:
```rust
for (_file_path, mut intent_ids) in intents_by_file {
    self.complete_graph_stage_without_sync(store, &mut intent_ids);
}
```
With:
```rust
self.batch_complete_graph_stage_without_sync(store, intents_by_file);
```

In `process_quality_batch`, apply the same replacement — but ONLY when `self.skip_surreal_sync` is true. When it's false, the existing `finalize_graph_stage` path (which does graph sync) must remain unchanged.

### Step 7: Keep `upsert_file_rollup` for non-bulk callers

The per-file serial `upsert_file_rollup` is still used by the watcher path and `process_event_with_priority_and_pass_and_overrides`. Do NOT remove it.

## Scope Guard

**Files modified:**
- `crates/aether-store/src/write_intents.rs` — add `batch_complete_intents` method
- `crates/aetherd/src/sir_pipeline/mod.rs` — add `bulk_upsert_file_rollups`, `batch_complete_graph_stage_without_sync`, replace serial loops in `process_bulk_scan` and `process_quality_batch`
- `crates/aetherd/src/sir_pipeline/rollup.rs` — add async-compatible rollup summarization helper or `build_file_rollup_prompt` helper

**Files NOT modified:**
- No changes to CLI
- No changes to config
- No schema changes
- No changes to batch pipeline
- No changes to provider implementations
- `upsert_file_rollup` and `complete_graph_stage_without_sync` preserved for non-bulk callers

## Validation

```bash
cargo fmt --all --check

cargo clippy -p aetherd --features dashboard -- -D warnings
cargo clippy -p aether-store -- -D warnings

# Per-crate tests — Do NOT run cargo test --workspace
cargo test -p aetherd
cargo test -p aether-store
```

Do NOT run `cargo test --workspace` — OOM risk on Codex.

## Commit

```
perf(sir_pipeline): parallel file rollup inference + batched graph completion

Replace the serial per-file rollup loop with concurrent API calls using
JoinSet+Semaphore. Files with ≤5 symbols use deterministic concatenation
(no API call). Files with >5 symbols submit rollup summarization calls
concurrently at the configured concurrency level.

Replace per-intent graph completion writes with a single-transaction
batch_complete_intents that advances all intents from current status
through GraphDone to Complete in one SQLite transaction.

For 380 files with ~200 requiring API calls and 5,443 intents:
Before: 8-14 min per pass (serial API calls + serial SQLite writes)
After: ~30 sec per pass (concurrent API + batched SQLite)

Applied to both process_bulk_scan and process_quality_batch.
Serial upsert_file_rollup preserved for watcher/single-file callers.
```

## Post-fix Cleanup

```bash
git push origin feature/parallel-rollup
```

Create PR via GitHub web UI with title and body from commit message above.

After merge:
```bash
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/parallel-rollup
git branch -D feature/parallel-rollup
```

## PR Title

`perf(sir_pipeline): parallel file rollups + batched graph completion`

## PR Body

Replace the serial per-file rollup loop with concurrent API calls using
JoinSet+Semaphore. Files with ≤5 symbols use deterministic concatenation
(no API call). Files with >5 symbols submit rollup summarization calls
concurrently at the configured inference concurrency.

Replace per-intent serial graph completion with `batch_complete_intents` —
a single SQLite transaction for all intents per pass.

For 380 files and 5,443 intents per pass:
- Before: 8-14 min per pass (serial API + serial SQLite)
- After: ~30 sec per pass (concurrent API + batched SQLite)

Applied to both `process_bulk_scan` and `process_quality_batch`.
Serial `upsert_file_rollup` preserved for watcher/single-file paths.
