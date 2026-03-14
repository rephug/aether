# Phase 10 — The Conductor

## Stage 10.1 — Batch Index Pipeline + Watcher Intelligence

### Purpose

Two improvements shipped together because both touch `AetherConfig` and both feed Phase 10.2:

**Async Batch Indexing** — decouples symbol extraction from SIR inference for cold-start indexing. Submit thousands of symbols to Gemini's Batch API in one JSONL file, walk away, ingest results at 50% cost with higher rate limits. Prompt hashing ensures only symbols whose context actually changed are submitted — reducing repeat batch costs by 90%+ on quiet days.

**Watcher Intelligence Upgrade** — the existing file-watcher fires on every save with the default inference model. This upgrade adds a separate high-quality model for real-time SIR generation on actively edited symbols, plus git operation triggers that automatically re-index changed files on branch switch, pull, merge, and rebase.

### What Problem This Solves

**Cold start:** A 10K-symbol codebase costs ~$2 and takes hours via real-time inference (one API call per symbol, sequential). Gemini Batch API processes the same workload at $1, asynchronously, with higher throughput. For nightly re-generation on the netcup servers, batch is the only practical path.

**Redundant re-inference:** Without prompt hashing, a nightly triage rebuild sends 49,000 identical prompts for symbols that haven't changed. Prompt hashing (`blake3(source_hash + neighbor_sir_hashes + config_hash)`) skips these entirely, reducing a $1 batch to $0.05 on quiet days.

**Stale context on active edits:** When you save a file and immediately ask an AI agent about the function you just changed, the SIR served via MCP is from the last index run — potentially hours old. A premium real-time model (Sonnet-class) on file-save events keeps actively edited symbols fresh at negligible cost (5-20 symbols/hour × $0.005/symbol = $0.02-0.10/hour).

**Git blindness:** Switching branches, pulling changes, or merging a PR can change hundreds of files. The current watcher doesn't detect these — only individual file saves. Git triggers close this gap by diffing HEAD states via `gix` and re-indexing only the changed files.

### Architecture

```
Cold Start (batch path):
  aetherd batch extract  → tree-sitter scan, populate symbols table
  aetherd batch build    → compute prompt_hash per symbol, skip unchanged,
                           write batch_input.jsonl per pass
  Gemini Batch API       → async processing (shell script or inline submit)
  aetherd batch ingest   → read results JSONL, upsert SIRs + embeddings
  aetherd batch run      → orchestrates extract→build→submit→poll→ingest
                           with strict task barriers between passes

Real-Time (watcher path):
  File save event        → tree-sitter re-parse → changed symbols only
                         → watcher model (configurable, premium) → SIR upsert
  Git operation          → 3s settling debounce → gix diff old HEAD vs new HEAD
                         → union with dirty working directory files
                         → same watcher model → SIR upsert for affected symbols
```

### New Config: `[batch]`

Added to `AetherConfig` in `crates/aether-config/`:

```toml
[batch]
# Per-pass model API strings. Empty = require --scan-model flag at runtime.
scan_model = ""
triage_model = ""
deep_model = ""

# Thinking level per pass: "low", "medium", "high", "dynamic"
scan_thinking = "low"
triage_thinking = "medium"
deep_thinking = "high"

# Neighbor context depth for triage/deep prompts
# 1 = direct callers/callees, 2 = two hops
triage_neighbor_depth = 1
deep_neighbor_depth = 2

# Max symbol source chars per request (0 = no cap)
scan_max_chars = 10000
triage_max_chars = 10000
deep_max_chars = 0

# Passes to run in sequence with `batch run`
passes = ["scan"]

# Auto-submit next pass when previous completes
auto_chain = true

# JSONL output directory (default: {workspace}/.aether/batch/)
batch_dir = ""

# Gemini poll interval in seconds
poll_interval_secs = 60

# JSONL chunk size for error isolation (lines per file)
jsonl_chunk_size = 5000
```

### New Config: `[watcher]`

```toml
[watcher]
# Model for real-time SIR generation on file-save events.
# Separate from batch — actively edited symbols deserve best quality.
# Empty = use existing [inference] provider.
realtime_model = ""
realtime_provider = ""

# Git operation triggers
trigger_on_branch_switch = true
trigger_on_git_pull = true
trigger_on_merge = true

# When true, only re-index files changed between HEAD states.
# When false, full workspace re-index (use after major rebases).
git_trigger_changed_files_only = true

# Debounce settling time for git operations (seconds)
git_debounce_secs = 3.0

# Stub for Phase 10.2
trigger_on_build_success = false
```

### In scope

#### Prompt hashing (Deep Think findings G1 + #4 fix)

Each symbol's inference prompt is determined by: its source code, its neighbors' SIR intents (for triage/deep), and the generation config (model, thinking level, max chars). The hash MUST be a composite of individual sub-hashes so the fingerprint history can decompose which component triggered a change. A single finalized cryptographic hash cannot be reversed.

```
source_hash    = blake3(source_content)[..16]
neighbor_hash  = blake3(sorted(neighbor_sir_intents, newline-delimited))[..16]
config_hash    = blake3("{model}:{thinking}:{max_chars}")[..16]
prompt_hash    = "{source_hash}|{neighbor_hash}|{config_hash}"
```

Decomposition: split on `|` and compare each segment to determine if source, neighbors, or config changed.

Store `prompt_hash` in the `sir` SQLite table (the table storing SIR records — primary key is `id`, NOT `symbol_id`). During `batch build`, compute the current prompt hash and compare against stored. If they match, skip the symbol — its SIR is already based on identical input. This is the single biggest cost optimization in the entire pipeline.

New column added via migration (use `ensure_sir_column` helper in `schema.rs`, NOT raw ALTER TABLE):
```rust
// In schema.rs, if version < 8:
ensure_sir_column(conn, "prompt_hash", "TEXT")?;
```

#### Change fingerprint history

New table for tracking the *why* and *how much* of each SIR regeneration:

```sql
CREATE TABLE IF NOT EXISTS sir_fingerprint_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    -- What changed in the prompt hash
    trigger TEXT NOT NULL,           -- "batch_scan", "batch_triage", "batch_deep",
                                     -- "watcher", "inject", "drift_monitor"
    source_changed INTEGER NOT NULL, -- 0 or 1
    neighbor_changed INTEGER NOT NULL, -- 0 or 1
    config_changed INTEGER NOT NULL, -- 0 or 1
    -- Components
    prompt_hash_old TEXT,            -- NULL on first generation
    prompt_hash_new TEXT NOT NULL,
    -- Semantic shift magnitude
    delta_sem REAL,                  -- cosine distance old→new embedding, NULL on first gen
    -- Context
    model TEXT NOT NULL,             -- model that produced the new SIR
    generation_pass TEXT NOT NULL,   -- "scan", "triage", "deep", "injected"
    commit_hash TEXT                 -- git HEAD at time of generation
);

CREATE INDEX IF NOT EXISTS idx_fingerprint_symbol_time
    ON sir_fingerprint_history(symbol_id, timestamp DESC);

CREATE INDEX IF NOT EXISTS idx_fingerprint_delta
    ON sir_fingerprint_history(delta_sem DESC)
    WHERE delta_sem IS NOT NULL;
```

This complements the existing `sir_history` table (which stores the SIR content) by capturing *what caused* each regeneration and *how much meaning shifted*. The `sir_history` tells you what the SIR said at each version. The fingerprint history tells you why it changed and whether the change was significant.

**When to write a fingerprint row:**
- `batch ingest` — after upserting each SIR, compute Δ_sem if previous embedding exists, decompose which prompt hash component changed, write row
- Watcher SIR generation — same, with `trigger = "watcher"`
- `sir inject` (10.3) — write row with `trigger = "inject"`, Δ_sem computed from the inject
- Drift monitor re-queue (10.2) — write row with `trigger = "drift_monitor"` after re-generation

**Δ_sem computation:** Requires the old embedding. Two approaches:
1. Store `previous_embedding` as a blob column in `sir` table (simple but doubles embedding storage)
2. Fetch the symbol's current embedding from LanceDB/vector store BEFORE overwriting with the new one (one extra read per regeneration)

Recommendation: option 2. One LanceDB point-lookup per regeneration is negligible. No schema change to the vector store. **CRITICAL: The old embedding MUST be fetched BEFORE the new embedding is written, as LanceDB overwrites by symbol ID.**

#### Batch pipeline subcommands

- `aetherd batch extract` — tree-sitter only, populate symbols table, stop before inference
- `aetherd batch build --pass <scan|triage|deep>` — read symbols, compute prompt hash, skip unchanged, call existing `build_job()` from `sir_pipeline`, write JSONL. For triage/deep: inject neighbor intent context from graph using pre-fetch dictionary pattern
- `aetherd batch ingest --pass <scan|triage|deep> <results.jsonl>` — parse Gemini results, upsert SIR records, update prompt_hash, skip error lines without panic
- `aetherd batch run --passes <scan,triage,deep>` — end-to-end orchestrator with strict task barriers between passes

#### JSONL format

Each line in the input JSONL. **CRITICAL:** The `key` field encodes BOTH `symbol_id` and `prompt_hash` separated by `|`. The Gemini Batch API only returns the `key` field in responses — without encoding the prompt_hash here, `batch ingest` cannot recover it.

```json
{"key": "symbol_id_here|prompt_hash_here", "request": {"contents": [{"parts": [{"text": "...prompt..."}]}], "generationConfig": {"responseMimeType": "application/json", "thinkingConfig": {"thinkingLevel": "LOW"}}}}
```

Output JSONL from Gemini:
```json
{"key": "symbol_id_here|prompt_hash_here", "response": {"candidates": [{"content": {"parts": [{"text": "...SIR JSON..."}]}}]}}
```

`batch ingest` splits the returned `key` by `|` to recover both symbol_id and prompt_hash.

Chunk JSONL files at `jsonl_chunk_size` lines (default 5000, ~15MB). This limits blast radius of API/parsing faults and allows parallel uploads.

#### Neighbor context pre-fetch (Deep Think finding D1)

The N+1 query problem: 5000 triage symbols × 64 neighbors = 320,000 sequential SQLite SELECTs.

Fix: before building any prompts, traverse the DAG to collect all unique `neighbor_id`s into a `HashSet`. Bulk fetch in chunks of 900 (SQLite parameter limit): `SELECT id, sir_json FROM sir WHERE id IN (?, ?, ...)`. Build an in-memory `HashMap<String, String>`. Assemble all prompts purely from RAM.

#### Auto-chain barrier (Deep Think finding D2)

The orchestrator in `batch run` must `.await` the completion of `batch ingest` and verify the SQLite `tx.commit()` before spawning `batch build` for the next pass. Without this, triage builds prompts using stale scan data.

#### Batch submission

Shell script `scripts/gemini_batch_submit.sh` for the Gemini API HTTP workflow: upload JSONL → create batch job → poll status → download results. The Rust CLI generates the JSONL; the script handles the Gemini API calls.

Alternative: `aetherd batch run` with inline submission via `reqwest` (Gemini API supports inline requests for batches under 20MB). For larger jobs, use the file upload path.

#### Watcher upgrades

**Real-time model:** `[watcher] realtime_model` / `realtime_provider` — construct a one-off provider instance for file-save SIR jobs, separate from the batch provider.

**Git trigger debouncing (Deep Think finding E1):** Operation-settling debounce. When `.git/` changes are detected, start a timer (`git_debounce_secs`, default 3.0). If another event fires, reset the timer. When it expires, read the new SHA. Compare to `.aether/last_indexed_head`. If different, execute a single `gix diff`. Suppress regular file-watcher events during the settling window to prevent double-processing.

**File set union (Deep Think finding E2):** `gix diff` only sees committed changes. The developer may have dirty working-directory edits. Union both sources:

```
Files_target = gix_diff(HEAD_old, HEAD_new) ∪ notify_dirty_files
```

Clear the notify queue after extraction.

**Git operation detection:**
1. Watch `.git/HEAD` and `.git/refs/` for changes
2. On change, start debounce timer
3. When timer fires: read stored HEAD SHA from `.aether/last_indexed_head`
4. Diff old HEAD vs new HEAD via `gix`
5. If `git_trigger_changed_files_only = true`: feed only changed file paths to existing re-index pipeline
6. If false: trigger full workspace re-index
7. Update stored HEAD SHA

**Edge cases:** detached HEAD (points to SHA not ref), first checkout with no prior SHA (skip diff, full re-index), gix diff failure (log warning, fall back to full re-index).

#### Code reuse

- `build_job()` from `sir_pipeline/infer.rs` — import directly, do NOT duplicate
- `build_sir_prompt_for_kind()` from `sir_pipeline/infer.rs` — reuse for prompt construction
- Symbol selection logic from existing pipeline — same threshold/top-N/fallback behavior
- All SQLite calls wrapped in `spawn_blocking`
- BLAKE3 already in workspace dependencies (Decision #8)

### Out of scope

- Continuous drift monitoring (Stage 10.2)
- SIR staleness scoring (Stage 10.2)
- Nightly scheduled re-runs (Stage 10.2)
- Post-build trigger implementation (Stage 10.2)
- Dashboard visibility into batch job status
- Batch API for non-Gemini providers
- `sir context` / `sir inject` commands (Stage 10.3)

### Implementation Notes

#### sir_pipeline module structure

`sir_pipeline` has been split into a module: `mod.rs`, `infer.rs`, `persist.rs`, `rollup.rs`, `tests.rs`. The batch pipeline imports from `infer.rs` (prompt building, job construction) and `persist.rs` (SIR upsert). No new files in `sir_pipeline/` — the batch code lives in a new `batch/` module under `aetherd/src/`.

#### aether-config module structure

Config was refactored into modules: `root.rs`, `inference.rs`, `embeddings.rs`, `sir_quality.rs`, `planner.rs`, `health.rs`, `storage.rs`, `search.rs`, `analysis.rs`, `verification.rs`, `constants.rs`, `normalize.rs`, `validate.rs`. New `BatchConfig` and `WatcherConfig` structs go in new files `batch.rs` and `watcher.rs`, imported into `root.rs`.

#### Thinking levels in JSONL

Gemini 3.x models support `thinkingConfig.thinkingLevel` in `generationConfig`. Values: `THINKING_LEVEL_UNSPECIFIED`, `LOW`, `MEDIUM`, `HIGH`, `DYNAMIC`. Map from config strings to these enum values when writing JSONL.

### Dependencies

New crate dependencies for `aetherd`:
- `gix` — already in workspace (used by existing git operations)

New crate dependencies for `aether-config`:
- None — uses existing `serde`, `toml`

### Pass criteria

1. `batch extract` populates symbols table, exits without calling inference.
2. `batch build --pass scan` produces valid JSONL — one line per symbol with `key` and `request` fields.
3. `batch build` skips symbols whose `prompt_hash` matches the stored value. Verify by running twice: second run produces empty or near-empty JSONL.
4. `batch ingest --pass scan` upserts all valid SIR records and updates `prompt_hash`. Error lines skipped without panic.
5. Triage JSONL contains neighbor intent context assembled via pre-fetch dictionary (no N+1 queries — verify with SQLite query logging).
6. `batch run --passes scan,triage` auto-chains correctly: triage build starts only after scan ingest commits.
7. `--triage-thinking high` overrides config. JSONL contains `"thinkingLevel":"HIGH"`.
8. `--neighbor-depth 2` produces longer prompts than depth 1 for symbols with transitive neighbors.
9. With `[watcher] realtime_model` set, file-save events use the watcher model, not the default model. Verify via structured logs.
10. With `trigger_on_branch_switch = true`, switching git branch triggers re-indexing of changed files after debounce settles (within 5 seconds of checkout).
11. File events during git debounce window are suppressed (no double-processing).
12. JSONL files are chunked at `jsonl_chunk_size` lines.
13. `sir_fingerprint_history` table is populated after batch ingest — verify rows exist with correct `trigger`, `source_changed`/`neighbor_changed`/`config_changed` flags, and `delta_sem` values.
14. Second batch ingest of same symbols shows `delta_sem ≈ 0.0` (identical SIRs produce near-zero semantic shift).
15. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
16. `cargo test -p aether-config` and `cargo test -p aetherd` pass.

### Estimated Codex runs: 2–3
