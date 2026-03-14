# Phase 10.1 — Batch Index Pipeline + Watcher Intelligence — Session Context

**Date:** 2026-03
**Branch:** `feature/phase10-stage10-1-batch-watcher` (to be created)
**Worktree:** `/home/rephu/aether-phase10-batch-watcher` (to be created)
**Starting commit:** HEAD of main (verify with `git log --oneline -1`)

## CRITICAL: Read actual source, not this document

```bash
# The live repo is at:
/home/rephu/projects/aether

# Always grep/read actual source before making claims about what exists
```

## Build environment (MUST be set for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

**Never run `cargo test --workspace`** — OOM risk. Always per-crate.

## What just merged (recent history)

Verify against actual `git log --oneline -10`. Expected recent commits:

| Commit | What |
|--------|------|
| (latest) | Phase 8.18 — Boundary Leaker false positive fix |
| f5bd037 | Batch embedding meta lookup (ARCH-2 N+1 fix) |
| b9834e4 | Batch progress logging for SIR generation |
| f6150b8 | Triage/deep pass concurrency fix (14x speedup) |
| 4e0917c | Refactor aether-mcp (last of 5 God File refactors) |
| 9f07651 | Phase 8.17 — Gemini native embedding provider |

## What this stage adds

Two things:

### A. Batch pipeline (`aetherd batch` subcommands)

New CLI subcommands for cold-start indexing via Gemini Batch API:
- `batch extract` — tree-sitter only, populate symbols, stop
- `batch build --pass scan|triage|deep` — generate JSONL with prompt hashing
- `batch ingest --pass scan|triage|deep <file.jsonl>` — parse results, upsert SIRs
- `batch run --passes scan,triage,deep` — orchestrate end-to-end with auto-chaining

### B. Watcher intelligence

- `[watcher]` config: `realtime_model`/`realtime_provider` for premium file-save SIR
- Git triggers: watch `.git/HEAD` for branch switch/pull/merge, 3s settling debounce
- File set union: `gix_diff(HEAD_old, HEAD_new) ∪ notify_dirty_files`

### C. Change fingerprint history

New `sir_fingerprint_history` SQLite table logging prompt_hash, change source decomposition (source/neighbor/config), and Δ_sem for every SIR regeneration event.

---

## Current codebase architecture

### Workspace crates (16 total)

```
aether-analysis    aether-core       aether-config     aether-dashboard
aether-document    aether-graph-algo aether-health     aether-infer
aether-lsp         aether-mcp        aether-memory     aether-parse
aether-query       aether-sir        aether-store      aetherd
```

### Key files to understand before implementing

**Config (where new structs go):**
- `crates/aether-config/src/root.rs` — `AetherConfig` struct, add `batch: Option<BatchConfig>`, `watcher: Option<WatcherConfig>` (field MUST be named `watcher` not `watcher_config` so `[watcher]` TOML section deserializes correctly)
- `crates/aether-config/src/` — modular: `inference.rs`, `embeddings.rs`, `sir_quality.rs`, `planner.rs`, `health.rs`, `storage.rs`, etc.
- New files: `crates/aether-config/src/batch.rs`, `crates/aether-config/src/watcher.rs`

**CLI (where subcommands are wired):**
- `crates/aetherd/src/cli.rs` — `Commands` enum (line ~444), `Cli` struct (line ~486)
- `crates/aetherd/src/main.rs` — `run_subcommand()` dispatch (line ~302)

**SIR pipeline (reuse, do NOT duplicate):**
- `crates/aetherd/src/sir_pipeline/mod.rs` — public API
- `crates/aetherd/src/sir_pipeline/infer.rs` — `build_job()`, `build_sir_prompt_for_kind()`, prompt construction
- `crates/aetherd/src/sir_pipeline/persist.rs` — SIR upsert to SQLite + mirror
- `crates/aetherd/src/sir_pipeline/rollup.rs` — file/module rollups

**Indexer (watcher lives here):**
- `crates/aetherd/src/indexer.rs` — file watcher, event handling, re-index dispatch (2044 lines)

**Inference providers:**
- `crates/aether-infer/src/lib.rs` — `InferenceProviderKind`: Auto, Tiered, Gemini, Qwen3Local, OpenAiCompat
- Provider factory handles all kinds — reuse for watcher one-off provider

**Git operations:**
- `gix` is already in workspace dependencies
- Existing usage in `crates/aetherd/src/` for blame/log operations

### SQLite schema (relevant tables)

```sql
-- Existing (table is named 'sir', NOT 'sir_meta'. Primary key is 'id', NOT 'symbol_id'.)
symbols (id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at, ...)
sir (id, sir_hash, sir_version, provider, model, updated_at, sir_json, sir_status, last_error, last_attempt_at, generation_pass, prompt_hash)
sir_history (symbol_id, version, sir_hash, provider, model, ...)
symbol_edges (source_id, target_id, edge_kind, file_path, ...)
project_notes (id, note_text, file_ref, ...)
test_intents (...)
coupling_mining_state (...)

-- NOTE: Coupling data (co_change edges) is in SurrealDB, NOT SQLite.
-- Access via graph_store.list_co_change_edges_for_file() or list_top_co_change_edges()

-- New (this stage — added in schema migration v8)
sir_fingerprint_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    symbol_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    prompt_hash TEXT NOT NULL,        -- composite: "source_hash|neighbor_hash|config_hash"
    prompt_hash_previous TEXT,
    trigger TEXT NOT NULL,            -- 'batch_scan', 'batch_triage', 'batch_deep', 'watcher', 'inject'
    source_changed INTEGER NOT NULL DEFAULT 0,
    neighbor_changed INTEGER NOT NULL DEFAULT 0,
    config_changed INTEGER NOT NULL DEFAULT 0,
    generation_model TEXT,
    generation_pass TEXT,
    delta_sem REAL                    -- cosine distance old→new embedding, NULL on first gen
)
```

### Production config (current)

```toml
[inference]
provider = "gemini"
model = "gemini-3.1-flash-lite-preview"
api_key_env = "GEMINI_API_KEY"
concurrency = 12

[embeddings]
enabled = true
provider = "gemini_native"
model = "gemini-embedding-2-preview"
api_key_env = "GEMINI_API_KEY"
dimensions = 3072
vector_backend = "sqlite"

[planner]
semantic_rescue_threshold = 0.90
```

### SurrealKV lock contention

Dashboard and CLI cannot share a workspace simultaneously. `pkill -f aetherd` before running CLI commands if the dashboard is running.

---

## Scope guard — do NOT modify

- Existing real-time pipeline behavior (only ADD watcher config, don't change defaults)
- Existing CLI subcommands
- `sir_pipeline/infer.rs` — reuse only, change visibility if needed (pub(crate))
- `sir_pipeline/persist.rs` — reuse only
- Any existing config fields (only ADD new optional fields)

---

## End-of-stage sequence

```bash
# Push and create PR
cd /home/rephu/aether-phase10-batch-watcher
git push -u origin feature/phase10-stage10-1-batch-watcher

# After PR merges:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase10-batch-watcher
git branch -d feature/phase10-stage10-1-batch-watcher
```
