# Codex Prompt: Parallel Batch Submission — Submit All Chunks Simultaneously

## Context

Read `docs/roadmap/parallel_batch_session_context.md` for verified code references.

The batch pipeline (`aetherd batch run`) currently submits JSONL chunks **sequentially**: submit chunk 1, poll until done (~7 min queue wait), download, ingest, submit chunk 2, poll (~7 min), etc. For 16 chunks, this means ~112 minutes of cumulative queue waiting.

The fix: submit all chunks simultaneously, then poll all of them in a round-robin loop, ingesting each as it completes. The total wait becomes the slowest single chunk (~10 min) instead of the sum of all chunks.

```
Current:  [chunk1 7min][chunk2 7min][chunk3 7min]... = ~112 min
Parallel: [chunk1  7min]
          [chunk2  8min]  → wait for slowest = ~10 min total
          [chunk3  6min]
          ...all 16 at once
```

**This changes ONE function** in `crates/aetherd/src/batch/run.rs`: `run_full_batch_command`. No trait changes, no provider changes, no config changes.

## Preflight

```bash
git status --porcelain
# Should be empty

git pull --ff-only

git worktree add -B feature/parallel-batch /home/rephu/feature/parallel-batch

cd /home/rephu/feature/parallel-batch

export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Mandatory Source Inspection

Before writing any code, inspect these files and answer the questions:

1. Read `crates/aetherd/src/batch/run.rs` function `run_full_batch_command` (line 92). Identify:
   - The sequential chunk loop: `for input_jsonl in build_summary.files { submit_and_wait... ingest... }`
   - How `submit_and_wait` blocks on a single chunk until completion
   - The outer `for pass in passes` loop — each pass (scan, triage, deep) must still be sequential (triage depends on scan results), but chunks WITHIN a pass can be parallel

2. Read `crates/aetherd/src/batch/run.rs` function `submit_and_wait` (line 189). Identify:
   - It returns `Result<Vec<PathBuf>>` (paths to downloaded result files)
   - It calls `provider.submit()` → `provider.poll()` in a loop → `provider.download_results()`
   - The poll loop sleeps `runtime.poll_interval_secs` between checks

3. Read `crates/aetherd/src/batch/mod.rs` trait `BatchProvider`. Identify:
   - `submit()` returns `Vec<String>` (job IDs, usually one per chunk)
   - `poll()` takes `&[String]` but only checks the FIRST job ID in all three implementations
   - `download_results()` takes `&[String]` but only uses the FIRST job ID
   - This means each chunk's job_ids must be polled/downloaded independently

4. Check all three provider `poll` implementations (gemini.rs, openai.rs, anthropic.rs). Confirm each only uses `job_ids.first()`.

## Implementation

### Step 1: Add a `PendingBatchJob` struct in `run.rs`

At the top of the file (after imports):

```rust
/// Tracks a submitted batch chunk awaiting completion.
struct PendingBatchJob {
    /// Job IDs returned by the provider's submit call.
    job_ids: Vec<String>,
    /// Path to the JSONL input file (for error messages).
    input_path: PathBuf,
    /// Index in the original chunk list (for ordering log messages).
    chunk_index: usize,
}
```

### Step 2: Add an `async fn submit_all_chunks` function

This replaces the inner `for input_jsonl in build_summary.files` loop. It submits all chunks without waiting:

```rust
/// Submit all JSONL chunks to the batch provider without waiting for completion.
/// Returns a list of pending jobs to be polled.
async fn submit_all_chunks(
    provider: &dyn BatchProvider,
    input_files: Vec<PathBuf>,
    model: &str,
    runtime: &BatchRuntimeConfig,
) -> Result<Vec<PendingBatchJob>> {
    let mut pending = Vec::with_capacity(input_files.len());

    for (index, input_path) in input_files.into_iter().enumerate() {
        let job_ids = provider
            .submit(&input_path, model, &runtime.batch_dir, runtime.poll_interval_secs)
            .await
            .with_context(|| format!("batch submit failed for chunk {}", input_path.display()))?;

        tracing::info!(
            provider = provider.name(),
            chunk = index + 1,
            jobs = ?job_ids,
            "submitted batch chunk"
        );

        pending.push(PendingBatchJob {
            job_ids,
            input_path,
            chunk_index: index,
        });
    }

    tracing::info!(
        total_chunks = pending.len(),
        "all batch chunks submitted, polling for completion"
    );

    Ok(pending)
}
```

### Step 3: Add an `async fn poll_and_ingest_all` function

This polls all pending jobs in a round-robin loop, downloading and ingesting each as it completes:

```rust
/// Poll all pending batch jobs, downloading and ingesting results as each completes.
/// Returns the paths of all successfully downloaded result files.
async fn poll_and_ingest_all(
    provider: &dyn BatchProvider,
    mut pending: Vec<PendingBatchJob>,
    runtime: &BatchRuntimeConfig,
) -> Result<Vec<(usize, Vec<PathBuf>)>> {
    let mut completed: Vec<(usize, Vec<PathBuf>)> = Vec::new();

    while !pending.is_empty() {
        tokio::time::sleep(std::time::Duration::from_secs(runtime.poll_interval_secs)).await;

        let mut still_pending = Vec::new();

        for job in pending {
            match provider.poll(&job.job_ids).await? {
                BatchPollStatus::Completed => {
                    tracing::info!(
                        chunk = job.chunk_index + 1,
                        "batch chunk completed, downloading results"
                    );
                    let result_paths = provider
                        .download_results(&job.job_ids, &runtime.batch_dir)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to download results for chunk {}",
                                job.input_path.display()
                            )
                        })?;
                    completed.push((job.chunk_index, result_paths));
                }
                BatchPollStatus::Failed { message } => {
                    tracing::error!(
                        chunk = job.chunk_index + 1,
                        error = %message,
                        "batch chunk failed"
                    );
                    return Err(anyhow!(
                        "batch chunk {} failed: {}",
                        job.input_path.display(),
                        message
                    ));
                }
                BatchPollStatus::InProgress { completed: done, total } => {
                    if let (Some(c), Some(t)) = (done, total) {
                        tracing::info!(
                            chunk = job.chunk_index + 1,
                            completed = c,
                            total = t,
                            remaining_chunks = still_pending.len() + 1,
                            "batch chunk in progress"
                        );
                    } else {
                        tracing::info!(
                            chunk = job.chunk_index + 1,
                            remaining_chunks = still_pending.len() + 1,
                            "batch chunk in progress"
                        );
                    }
                    still_pending.push(job);
                }
            }
        }

        pending = still_pending;

        if !pending.is_empty() {
            tracing::info!(
                remaining = pending.len(),
                completed = completed.len(),
                "polling {} remaining batch chunks",
                pending.len()
            );
        }
    }

    Ok(completed)
}
```

### Step 4: Rewrite `run_full_batch_command` to use parallel submission

Replace the inner chunk loop in `run_full_batch_command`. The outer `for pass in passes` loop stays — passes must be sequential (triage needs scan results). Only chunk submission within a pass becomes parallel.

The current sequential pattern (lines ~128-155):
```rust
for input_jsonl in build_summary.files {
    let results_paths = tokio_rt.block_on(submit_and_wait(...))?;
    for result_path in results_paths {
        ingest_results(...)?;
    }
}
```

Replace with:
```rust
// Phase 1: Submit all chunks for this pass
let pending = tokio_rt.block_on(submit_all_chunks(
    provider.as_ref(),
    build_summary.files,
    &pass_config.model,
    &runtime,
))?;

// Phase 2: Poll all chunks, download as each completes
let completed = tokio_rt.block_on(poll_and_ingest_all(
    provider.as_ref(),
    pending,
    &runtime,
))?;

// Phase 3: Ingest all downloaded results
for (chunk_index, result_paths) in completed {
    for result_path in result_paths {
        let ingest_summary = ingest_results(
            workspace,
            &extract_summary.store,
            &pass_config,
            result_path.as_path(),
            config,
            provider.as_ref(),
            provider.name(),
        )?;
        println!(
            "Ingested {} chunk {}: processed {}, skipped {}, fingerprint rows {}",
            pass.as_str(),
            chunk_index + 1,
            ingest_summary.processed,
            ingest_summary.skipped,
            ingest_summary.fingerprint_rows
        );
    }
}
```

### Step 5: Remove or keep `submit_and_wait`

Check if `submit_and_wait` has any callers other than the old sequential loop. If not, remove it to avoid dead code warnings. If it's used elsewhere, leave it.

### Step 6: Handle the `anyhow` import

The `poll_and_ingest_all` function uses `anyhow!`. Verify it's already imported. If not, add it.

## Important Notes

**Passes are still sequential.** The `for pass in passes` loop must remain serial because triage depends on scan results. Only chunks within a single pass run in parallel.

**Provider batch creation limits:**
- Anthropic: 50 batch creation requests per minute. For 16 chunks, this is well under the limit.
- OpenAI: 50K requests / 200MB per batch file. No explicit batch creation rate limit.
- Gemini: No documented batch creation rate limit.

If a provider starts rejecting submissions due to rate limits during the submit phase, the existing error handling will surface it clearly.

**Ingest order doesn't matter.** Each chunk's results are independent — symbols don't depend on other symbols within the same pass. Ingesting chunk 7 before chunk 3 produces the same final state.

## Scope Guard

**Files modified:**
- `crates/aetherd/src/batch/run.rs` — add PendingBatchJob struct, add submit_all_chunks, add poll_and_ingest_all, rewrite inner loop of run_full_batch_command

**Files NOT modified:**
- No changes to BatchProvider trait
- No changes to provider implementations (gemini.rs, openai.rs, anthropic.rs)
- No changes to batch config
- No changes to ingest.rs or build.rs
- No CLI changes

## Validation

```bash
cargo fmt --all --check

cargo clippy -p aetherd --features dashboard -- -D warnings

# Per-crate tests — Do NOT run cargo test --workspace
cargo test -p aetherd
```

Do NOT run `cargo test --workspace` — OOM risk on Codex.

## Commit

```
perf(batch): parallel chunk submission — submit all JSONL chunks simultaneously

Replace the sequential submit→poll→ingest loop with parallel submission:
all chunks are submitted upfront, then polled in a round-robin loop with
results ingested as each chunk completes.

For 16 chunks with ~7 min average queue time each:
Before: 16 × 7 min = ~112 min (sequential)
After: max(queue times) = ~10 min (parallel)

Passes remain sequential (triage depends on scan). Only chunks within
a single pass are parallelized. Works with all three batch providers
(Gemini, OpenAI, Anthropic) without trait or provider changes.
```

## Post-fix Cleanup

```bash
git push origin feature/parallel-batch
```

Create PR via GitHub web UI with title and body from commit message above.

After merge:
```bash
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/parallel-batch
git branch -D feature/parallel-batch
```

## PR Title

`perf(batch): parallel chunk submission for 10x faster batch pipeline`

## PR Body

Replace the sequential submit→poll→ingest loop with parallel submission:
all chunks are submitted upfront, then polled in a round-robin loop with
results ingested as each chunk completes.

For 16 chunks with ~7 min average queue time each:
- Before: 16 × 7 min = ~112 min (sequential)
- After: max(queue times) = ~10 min (parallel)

Passes remain sequential (triage depends on scan results). Only chunks
within a single pass are parallelized. Works with all three batch providers
(Gemini, OpenAI, Anthropic) without trait or provider changes.
