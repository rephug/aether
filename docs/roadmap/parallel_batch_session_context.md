# Session Context: Parallel Batch Submission

## Problem

`run_full_batch_command` in `crates/aetherd/src/batch/run.rs` (line 92) submits JSONL chunks **sequentially**:

```rust
for input_jsonl in build_summary.files {
    let results_paths = tokio_rt.block_on(submit_and_wait(
        provider.as_ref(),
        &input_jsonl,
        &pass_config.model,
        &runtime,
    ))?;

    for result_path in results_paths {
        ingest_results(...)?;
    }
}
```

Each `submit_and_wait` call (line 189) submits one chunk, then polls in a loop until that chunk completes (~7 min queue wait per chunk). For 16 chunks, total time is ~112 minutes.

## Goal

Submit all 16 chunks simultaneously. Poll all in a round-robin loop. Ingest each as it completes. Total time becomes ~10 minutes (the slowest single chunk).

## Architecture Constraint

The `for pass in passes` outer loop MUST remain sequential. Triage depends on scan results being ingested first. Only chunks within a single pass can be parallelized.

## BatchProvider Trait (no changes needed)

```rust
// crates/aetherd/src/batch/mod.rs line 52
trait BatchProvider: Send + Sync {
    fn format_request(&self, ...) -> Result<String>;
    async fn submit(&self, input_path: &Path, model: &str, batch_dir: &Path, poll_interval_secs: u64) -> Result<Vec<String>>;
    async fn poll(&self, job_ids: &[String]) -> Result<BatchPollStatus>;
    async fn download_results(&self, job_ids: &[String], output_dir: &Path) -> Result<Vec<PathBuf>>;
    fn parse_result_line(&self, line: &str) -> Result<BatchResultLine>;
    fn name(&self) -> &str;
}

enum BatchPollStatus {
    InProgress { completed: Option<u64>, total: Option<u64> },
    Completed,
    Failed { message: String },
}
```

**Critical:** All three provider implementations (gemini.rs:215, openai.rs:180, anthropic.rs:192) only check `job_ids.first()` in their `poll` and `download_results` methods. Each chunk's job_ids must be polled independently — you cannot merge all job IDs into one vec and poll once.

## Provider-Specific Batch Limits

| Provider | Batch Creation Limit | Chunk Size Limit | Notes |
|---|---|---|---|
| Gemini | No documented limit | File upload size | Each submit does: upload file → create batch job |
| OpenAI | No explicit limit | 50K requests / 200MB per file | Each submit uploads file → creates batch |
| Anthropic | 50 batch creations/min | 10K requests / 32MB per file | 16 chunks is well under 50/min |

## `submit_and_wait` (line 189) — to be replaced

```rust
async fn submit_and_wait(
    provider: &dyn BatchProvider,
    input_path: &Path,
    model: &str,
    runtime: &BatchRuntimeConfig,
) -> Result<Vec<PathBuf>> {
    let job_ids = provider.submit(input_path, model, &runtime.batch_dir, runtime.poll_interval_secs).await?;
    
    loop {
        tokio::time::sleep(Duration::from_secs(runtime.poll_interval_secs)).await;
        match provider.poll(&job_ids).await? {
            BatchPollStatus::Completed => break,
            BatchPollStatus::Failed { message } => return Err(anyhow!("batch job failed: {}", message)),
            BatchPollStatus::InProgress { .. } => { /* log and continue */ }
        }
    }

    provider.download_results(&job_ids, &runtime.batch_dir).await
}
```

Only caller: `run_full_batch_command` line 135. Can be removed after refactor.

## `BatchRuntimeConfig` (line 145)

```rust
pub(crate) struct BatchRuntimeConfig {
    pub batch_dir: PathBuf,
    // ... pass configs ...
    pub poll_interval_secs: u64,  // default 60
}
```

## Ingest Independence

Each chunk's results are independent. `ingest_results` (batch/ingest.rs) processes one JSONL result file at a time, upserting SIRs and embeddings per symbol. Order doesn't matter — ingesting chunk 7 before chunk 3 produces identical final state.

## Observed Timing Data (from testing session)

| Provider | Chunk Size | Queue Wait | Processing | Total/Chunk |
|---|---|---|---|---|
| Gemini flash-lite | 350 | 3-6 min | 1-2 min | 4-8 min |
| OpenAI mini+reasoning | 350 | 4-5 min | 1-4 min | 5-9 min |
| Anthropic Haiku | 350 | 6-7 min | 1-2 min | 7-9 min |

With 16 chunks sequential: ~112 min. With 16 chunks parallel: ~10 min.
