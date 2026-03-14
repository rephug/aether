# Codex Prompt — Phase 10.1: Batch Index Pipeline + Watcher Intelligence

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Never `cargo test --workspace` — always per-crate.

Read these files before writing any code:
- `docs/roadmap/phase_10_stage_10_1_batch_index_v2.md` (the spec)
- `docs/roadmap/phase10_stage10_1_session_context.md` (session context — crate layout, key files, schema)
- `crates/aetherd/src/sir_pipeline/infer.rs` (build_job, build_sir_prompt_for_kind — REUSE these)
- `crates/aetherd/src/sir_pipeline/persist.rs` (SIR upsert — REUSE this)
- `crates/aetherd/src/cli.rs` (Commands enum — add new variants here)
- `crates/aetherd/src/main.rs` (run_subcommand — add dispatch here)
- `crates/aether-config/src/root.rs` (AetherConfig — add new fields here)
- `crates/aetherd/src/indexer.rs` (existing watcher — modify for git triggers)
- `crates/aether-store/src/schema.rs` (SQLite migrations — add new version here)
- `crates/aether-store/src/sir_meta.rs` (SirMetaRecord struct — add prompt_hash field)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase10-batch-watcher -b feature/phase10-stage10-1-batch-watcher
cd /home/rephu/aether-phase10-batch-watcher
```

## SOURCE INSPECTION

Before writing code, read and verify these assumptions. If any are false, STOP and report:

1. `Commands` enum in `cli.rs` uses `#[derive(Subcommand)]` — verify exact derive macro
2. `run_subcommand()` in `main.rs` dispatches via `match command { Commands::Foo(args) => ... }`
3. `sir_pipeline/infer.rs` exports `build_job()` — check visibility (may need `pub(crate)`)
4. `sir_pipeline/infer.rs` exports `build_sir_prompt_for_kind()` — check visibility
5. `sir_pipeline/persist.rs` has a function that upserts a SIR to SQLite — find its name and signature
6. `AetherConfig` in `root.rs` has `#[serde(default)]` on all fields
7. `gix` is in workspace `Cargo.toml` dependencies
8. `blake3` is in workspace `Cargo.toml` dependencies (used for symbol IDs)
9. **The SIR table is named `sir` (NOT `sir_meta`).** Primary key is `id` (NOT `symbol_id`). The Rust struct is `SirMetaRecord` in `crates/aether-store/src/sir_meta.rs`. Verify the column list: `id, sir_hash, sir_version, provider, model, updated_at, sir_json, sir_status, last_error, last_attempt_at, generation_pass`. Check if `prompt_hash` column already exists.
10. Schema migrations use `PRAGMA user_version`. Current version is 7. New migrations go in `if version < 8` blocks in `crates/aether-store/src/schema.rs`. Use `ensure_sir_column()` helper for adding columns to existing tables.
11. The `WalkBuilder` in `indexer.rs` uses `.hidden(true).git_ignore(true)` which strips `.git/` directories from the file watcher. Git trigger watches must be added SEPARATELY via direct `watcher.watch()` calls.

## IMPLEMENTATION

### Step 1: New config structs

Create `crates/aether-config/src/batch.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchConfig {
    #[serde(default)] pub scan_model: String,
    #[serde(default)] pub triage_model: String,
    #[serde(default)] pub deep_model: String,
    #[serde(default = "default_scan_thinking")] pub scan_thinking: String,
    #[serde(default = "default_triage_thinking")] pub triage_thinking: String,
    #[serde(default = "default_deep_thinking")] pub deep_thinking: String,
    #[serde(default = "default_triage_neighbor_depth")] pub triage_neighbor_depth: u32,
    #[serde(default = "default_deep_neighbor_depth")] pub deep_neighbor_depth: u32,
    #[serde(default = "default_scan_max_chars")] pub scan_max_chars: usize,
    #[serde(default = "default_triage_max_chars")] pub triage_max_chars: usize,
    #[serde(default)] pub deep_max_chars: usize,
    #[serde(default = "default_passes")] pub passes: Vec<String>,
    #[serde(default = "default_true")] pub auto_chain: bool,
    #[serde(default)] pub batch_dir: String,
    #[serde(default = "default_poll_interval")] pub poll_interval_secs: u64,
    #[serde(default = "default_jsonl_chunk_size")] pub jsonl_chunk_size: usize,
}
```

Defaults: `scan_thinking = "low"`, `triage_thinking = "medium"`, `deep_thinking = "high"`, `triage_neighbor_depth = 1`, `deep_neighbor_depth = 2`, `scan_max_chars = 10000`, `triage_max_chars = 10000`, `passes = ["scan"]`, `poll_interval_secs = 60`, `jsonl_chunk_size = 5000`.

Create `crates/aether-config/src/watcher.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatcherConfig {
    #[serde(default)] pub realtime_model: String,
    #[serde(default)] pub realtime_provider: String,
    #[serde(default = "default_true")] pub trigger_on_branch_switch: bool,
    #[serde(default = "default_true")] pub trigger_on_git_pull: bool,
    #[serde(default = "default_true")] pub trigger_on_merge: bool,
    #[serde(default = "default_true")] pub git_trigger_changed_files_only: bool,
    #[serde(default = "default_git_debounce")] pub git_debounce_secs: f64,
    #[serde(default)] pub trigger_on_build_success: bool,
}
```

Defaults: `git_debounce_secs = 3.0`, all triggers `true`, `trigger_on_build_success = false`.

Add to `AetherConfig` in `root.rs`:
```rust
#[serde(default)]
pub batch: Option<BatchConfig>,
#[serde(default, rename = "watcher")]
pub watcher: Option<WatcherConfig>,
```

**IMPORTANT:** The field MUST be named `watcher` (or use `#[serde(rename = "watcher")]`) so the TOML section `[watcher]` deserializes correctly. Do NOT name it `watcher_config`.

Register modules in `lib.rs`. Add unit tests: empty TOML deserializes without error, full TOML with `[batch]` and `[watcher]` sections parses correctly.

### Step 2: Schema migration — `prompt_hash` column + fingerprint history table

In `crates/aether-store/src/schema.rs`, add a new migration block:

```rust
if version < 8 {
    ensure_sir_column(conn, "prompt_hash", "TEXT")?;

    conn.execute_batch("
        CREATE TABLE IF NOT EXISTS sir_fingerprint_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            symbol_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            prompt_hash TEXT NOT NULL,
            prompt_hash_previous TEXT,
            trigger TEXT NOT NULL,
            source_changed INTEGER NOT NULL DEFAULT 0,
            neighbor_changed INTEGER NOT NULL DEFAULT 0,
            config_changed INTEGER NOT NULL DEFAULT 0,
            generation_model TEXT,
            generation_pass TEXT,
            delta_sem REAL
        );

        CREATE INDEX IF NOT EXISTS idx_fingerprint_symbol_time
            ON sir_fingerprint_history(symbol_id, timestamp DESC);

        CREATE INDEX IF NOT EXISTS idx_fingerprint_delta
            ON sir_fingerprint_history(delta_sem DESC)
            WHERE delta_sem IS NOT NULL;
    ")?;

    conn.execute("PRAGMA user_version = 8", [])?;
}
```

Also update `SirMetaRecord` in `crates/aether-store/src/sir_meta.rs`:
- Add `pub prompt_hash: Option<String>` field
- Update the `params!` array in `store_upsert_sir_meta` to include `prompt_hash`
- Update the SELECT query in `store_get_sir_meta` to read `prompt_hash`

### Step 3: Prompt hashing module (composite hash with sub-hashes)

Create `crates/aetherd/src/batch/hash.rs`:

**CRITICAL:** The prompt hash must be a composite of sub-hashes so we can later decompose which component changed. A single finalized BLAKE3 hash cannot be reversed to identify which input changed.

```rust
use blake3::Hasher;

/// Compute individual sub-hashes and combine into a decomposable composite.
/// Format: "source_hash|neighbor_hash|config_hash"
/// This allows fingerprint history to determine WHICH component triggered a change
/// by splitting on '|' and comparing each segment.
pub fn compute_prompt_hash(source: &str, neighbor_intents: &[&str], config: &str) -> String {
    let source_hash = {
        let mut h = Hasher::new();
        h.update(source.as_bytes());
        h.finalize().to_hex()[..16].to_string()
    };

    let neighbor_hash = {
        let mut h = Hasher::new();
        let mut sorted: Vec<&str> = neighbor_intents.to_vec();
        sorted.sort();
        for intent in &sorted {
            h.update(intent.as_bytes());
            h.update(b"\n"); // delimiter to prevent collision between ["ab","c"] and ["a","bc"]
        }
        h.finalize().to_hex()[..16].to_string()
    };

    let config_hash = {
        let mut h = Hasher::new();
        h.update(config.as_bytes());
        h.finalize().to_hex()[..16].to_string()
    };

    format!("{source_hash}|{neighbor_hash}|{config_hash}")
}

/// Decompose a composite prompt hash into its three components.
pub fn decompose_prompt_hash(hash: &str) -> (Option<&str>, Option<&str>, Option<&str>) {
    let parts: Vec<&str> = hash.split('|').collect();
    (parts.get(0).copied(), parts.get(1).copied(), parts.get(2).copied())
}

/// Compare two composite hashes and return which components changed.
pub fn diff_prompt_hashes(old: &str, new: &str) -> (bool, bool, bool) {
    let (os, on, oc) = decompose_prompt_hash(old);
    let (ns, nn, nc) = decompose_prompt_hash(new);
    (os != ns, on != nn, oc != nc) // (source_changed, neighbor_changed, config_changed)
}
```

The `config` fingerprint string is: `format!("{model}:{thinking}:{max_chars}")` for the relevant pass.

### Step 4: Batch module

Create `crates/aetherd/src/batch/`:
- `mod.rs` — re-exports
- `extract.rs` — reuse existing tree-sitter file-walk pipeline, stop after symbol upsert
- `build.rs` — for each symbol in pass:
  1. Compute prompt hash via `compute_prompt_hash()`
  2. Compare against stored `sir.prompt_hash` (query the `sir` table by `id`)
  3. If match → skip
  4. If different → call `build_job()` from `sir_pipeline/infer.rs`, write JSONL line
  5. **CRITICAL: Encode the prompt hash into the JSONL key field** as `"{symbol_id}|{prompt_hash}"`. The Gemini Batch API only returns the `key` field in responses — without this, `batch ingest` cannot recover the prompt hash.
  - For triage/deep: use **pre-fetch dictionary** pattern:
    a. Collect all unique neighbor_ids into `HashSet`
    b. Bulk fetch in chunks of 900: `SELECT id, sir_json FROM sir WHERE id IN (?...)`
    c. Build `HashMap<String, String>` in memory
    d. Assemble prompts from RAM — zero N+1 queries
- `ingest.rs` — read JSONL lines, for each line:
  1. Split the returned `key` by `|` to recover `symbol_id` and `prompt_hash`
  2. Parse the SIR JSON from the response
  3. Upsert SIR to the `sir` table (via existing persist functions)
  4. Update `prompt_hash` on the `sir` row
  5. **If `config.embeddings.enabled`:** call the embedding refresh function (find the existing `refresh_embedding_if_needed` or equivalent in `sir_pipeline/mod.rs`) to generate the new embedding. This MUST happen BEFORE computing `delta_sem`.
  6. Compute `delta_sem` if old embedding exists (fetch old embedding BEFORE the refresh in step 5)
  7. Write fingerprint history row using `diff_prompt_hashes()` to decompose the change source
  8. Skip error lines without panic, log the error
- `run.rs` — orchestrator: extract → build → submit → poll → ingest
  - **Strict task barriers:** `.await` ingest completion + verify SQLite commit before spawning next pass build
  - Chunk JSONL at `jsonl_chunk_size` lines

### Step 5: CLI wiring

Add to `Commands` enum in `cli.rs`:

```rust
/// Batch indexing operations
Batch(BatchArgs),
```

Where `BatchArgs` has a nested subcommand:
```rust
#[derive(Debug, Clone, Subcommand)]
pub enum BatchCommand {
    Extract,
    Build(BatchBuildArgs),
    Ingest(BatchIngestArgs),
    Run(BatchRunArgs),
}
```

With args for `--pass`, `--passes`, `--scan-model`, `--triage-model`, `--deep-model`, `--triage-thinking`, `--neighbor-depth`, and the JSONL file path for ingest.

Wire dispatch in `run_subcommand()` in `main.rs`.

### Step 6: Watcher git triggers

In `crates/aetherd/src/indexer.rs`:

**CRITICAL:** The existing `WalkBuilder::new().hidden(true).git_ignore(true)` strips `.git/` from the watcher. You MUST add git directory watches SEPARATELY, outside the WalkBuilder loop.

1. After the WalkBuilder loop that registers source file watches, add explicit watches:
   ```rust
   // Watch .git/HEAD for branch switches, pulls, merges
   // This MUST be outside the WalkBuilder loop because WalkBuilder
   // uses .hidden(true).git_ignore(true) which excludes .git/
   let git_head = config.workspace.join(".git/HEAD");
   if git_head.exists() {
       watcher.watch(&config.workspace.join(".git"), RecursiveMode::Recursive)?;
   }
   ```

2. In the event handler, detect `.git/` events separately from source file events:
   - When `.git/` events fire, start a debounce timer (`git_debounce_secs`, default 3.0). Reset on each new event.
   - **Suppress normal file-change processing** during the settling window to prevent double-processing.
   - When timer fires:
     a. Read `.aether/last_indexed_head` (create if missing)
     b. Read current HEAD SHA
     c. If different: `gix diff` old→new HEAD to get changed file paths
     d. Union with any dirty working directory files from the notify queue
     e. Feed the union set to the existing re-index pipeline
     f. Write new HEAD SHA to `.aether/last_indexed_head`

3. Handle edge cases:
   - Detached HEAD: read SHA directly from `.git/HEAD` content
   - No prior SHA (`.aether/last_indexed_head` doesn't exist): skip diff, full re-index
   - gix diff failure: log warning, fall back to full re-index

### Step 7: Watcher model override

When `[watcher] realtime_model` is non-empty:

1. Construct a one-off `InferenceProvider` using the watcher model/provider config
2. Use this provider for file-save SIR generation instead of the default `[inference]` provider
3. The batch pipeline still uses `[batch]` models — completely separate

### Step 8: Fingerprint history writing

Create a helper function `write_fingerprint_row(...)` that:
1. Reads the previous `prompt_hash` from the `sir` table for this symbol (by `id`)
2. Calls `diff_prompt_hashes(old, new)` to decompose which component changed
3. Computes `delta_sem` if embeddings are enabled and both old and new embeddings exist
4. Inserts a row into `sir_fingerprint_history`

Call this helper from:
- `batch ingest` (after upserting each SIR and refreshing embedding)
- Watcher regeneration (after upserting each SIR)

## SCOPE GUARD — Do NOT modify

- Existing real-time pipeline behavior (only ADD watcher config, don't change defaults)
- Existing CLI subcommands
- `sir_pipeline/infer.rs` — reuse only, change visibility if needed (pub(crate))
- `sir_pipeline/persist.rs` — reuse only
- Any existing config fields (only ADD new optional fields)

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aether-config -- -D warnings
cargo clippy -p aether-store -- -D warnings
cargo clippy -p aetherd -- -D warnings
cargo test -p aether-config
cargo test -p aether-store
cargo test -p aetherd
```

Then verify CLI wiring (no API key needed):
```bash
./target/debug/aetherd batch --help
./target/debug/aetherd batch extract --help
./target/debug/aetherd batch build --help
./target/debug/aetherd batch ingest --help
./target/debug/aetherd batch run --help
```

### Validation criteria

1. All tests pass, zero clippy warnings
2. `batch --help` shows all four subcommands
3. Config tests: empty TOML parses, full `[batch]` + `[watcher]` TOML parses
4. Schema migration: version bumps to 8, `sir` table has `prompt_hash` column, `sir_fingerprint_history` table exists
5. Prompt hashing: two identical calls to `compute_prompt_hash()` with same inputs produce same output; different inputs produce different output; `diff_prompt_hashes()` correctly identifies which component changed
6. `batch build` skips symbols whose composite prompt hash matches stored value. Verify by running twice: second run produces empty or near-empty JSONL.
7. JSONL `key` field format is `"symbol_id|prompt_hash"` — verify by inspecting output file
8. `batch ingest` recovers both symbol_id and prompt_hash from the key field
9. `sir_fingerprint_history` rows populated after batch ingest with correct `source_changed`/`neighbor_changed`/`config_changed` flags
10. Watcher correctly watches `.git/HEAD` despite WalkBuilder's `.hidden(true)` exclusion
11. File events during git debounce window are suppressed (no double-processing)
12. JSONL files chunked at `jsonl_chunk_size` lines
13. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass
14. `cargo test -p aether-config`, `cargo test -p aether-store`, `cargo test -p aetherd` pass

## COMMIT

```bash
git add -A
git commit -m "Phase 10.1: Batch index pipeline + watcher intelligence

Batch pipeline:
- aetherd batch extract/build/ingest/run subcommands
- Gemini Batch API JSONL format with per-pass model and thinking level
- Prompt hashing via BLAKE3 composite (source|neighbor|config sub-hashes)
- JSONL key encodes symbol_id|prompt_hash for round-trip recovery
- Pre-fetch dictionary for neighbor context assembly (no N+1 queries)
- Auto-chaining with strict task barriers between passes
- JSONL chunking at configurable size for error isolation
- Embedding refresh in ingest path for delta_sem computation

Watcher intelligence:
- [watcher] realtime_model for premium file-save SIR generation
- Git triggers: .git/HEAD watched separately (bypasses WalkBuilder hidden filter)
- 3s settling debounce with file event suppression during git ops
- File set union: gix diff ∪ notify dirty files

Schema:
- Migration v8: prompt_hash column on sir table, sir_fingerprint_history table
- SirMetaRecord updated with prompt_hash field

Change fingerprint history:
- sir_fingerprint_history SQLite table with decomposable change source flags
- Logs prompt_hash, source/neighbor/config change flags, delta_sem per regeneration
- Instrumented in batch ingest and watcher regeneration paths"
```

**PR title:** Phase 10.1: Batch index pipeline + watcher intelligence
**PR body:** Adds async batch indexing via Gemini Batch API (50% cost reduction), prompt hashing to skip unchanged symbols, smarter file watcher with git operation triggers, and change fingerprint history tracking. New `[batch]` and `[watcher]` config sections. Schema migration v8.

Do NOT push automatically. Report the commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase10-stage10-1-batch-watcher
```
