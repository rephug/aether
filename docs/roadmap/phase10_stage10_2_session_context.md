# Phase 10.2 — Continuous Intelligence — Session Context

**Date:** 2026-03
**Branch:** `feature/phase10-stage10-2-continuous-intel` (to be created)
**Worktree:** `/home/rephu/aether-phase10-continuous` (to be created)
**Starting commit:** HEAD of main after 10.1 merged
**Prerequisite:** Stage 10.1 must be merged — this stage depends on `[batch]` config, `batch build`/`ingest` machinery, prompt hashing, and `sir_fingerprint_history` table.

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether
# Always grep/read actual source before making claims
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

## What 10.1 added (verify against actual repo)

- `crates/aether-config/src/batch.rs` — `BatchConfig` struct
- `crates/aether-config/src/watcher.rs` — `WatcherConfig` struct
- `crates/aetherd/src/batch/` — `extract.rs`, `build.rs`, `ingest.rs`, `run.rs`, `hash.rs`
- `sir_fingerprint_history` SQLite table
- `sir.prompt_hash` column (added by 10.1 migration v8)
- `[batch]` and `[watcher]` config sections in `AetherConfig`
- Git trigger debouncing in `indexer.rs`

## What this stage adds

### A. Staleness scoring

Noisy-OR formula with hard gates:
```
S_total = max(S_source, S_model, 1 - (1 - S_time)(1 - S_neighbor))
```

- `S_source ∈ {0, 1}` — source code changed since SIR generation
- `S_model ∈ {0, 1}` — model deprecated (lookup table)
- `S_time = sigmoid(t_effective)` with logistic decay, half-life 15 days
- `S_neighbor` — semantic-gated: `S_B × γ × Δ_sem(B)`, BFS with 0.1 cutoff

Cold-start volatility prior: `t_effective = t × (1 + log₂(1 + git_churn_30d))`

Priority: `S_total + 0.2 × log₁₀(1 + PR) / log₁₀(1 + PR_max)`

### B. Drift monitor

Background tokio task on schedule. Loads `symbol_edges` into `petgraph::DiGraph` once per run (~5MB for 50K nodes, <15ms traversal). Computes staleness for all symbols, selects top N, writes re-queue JSONL via 10.1 `batch build`.

### C. Predictive staleness

If `coupling(A, B) > 0.85` and A was just edited, bump B's staleness: `S_B = max(S_B, coupling × 0.5)`.

### D. Fingerprint history consumption

Read `sir_fingerprint_history` for volatility detection (≥3 events with Δ_sem > 0.2 in 30 days = volatile zone bump).

---

## Key files to understand

**Graph data (for in-memory loading):**
- **Structural edges are in BOTH SurrealDB and SQLite.** Use SQLite `symbol_edges` table for the drift monitor to avoid SurrealKV lock contention. Access via `store.get_callers()` and `store.get_dependencies()`.
- Do NOT query SurrealDB for structural edges in the drift monitor — if the daemon is running, SurrealKV's exclusive lock will crash the CLI.

**Health scoring (for churn data):**
- **`git_churn_30d` is NOT pre-computed globally.** There is no cached churn table.
- Computed per-file dynamically via `aether_health::git_signals::compute_file_git_stats()`
- Must instantiate `aether_core::git::GitContext`, group symbols by `file_path`, call per file
- Map resulting `commits_30d` value to all symbols within that file

**Embeddings (for Δ_sem):**
- LanceDB stores embeddings
- `crates/aether-store/src/` — embedding read/write functions
- For Δ_sem: read from `sir_fingerprint_history.delta_sem` for recently regenerated neighbors. Do NOT recompute embeddings during the monitor run.

**Coupling data:**
- **Coupling data is in SurrealDB, NOT SQLite.** There is NO `coupling_pairs` SQLite table.
- Access via `graph_store.list_co_change_edges_for_file()` or `list_top_co_change_edges()`
- For nightly cron (no daemon running), SurrealDB is accessible
- Existing coupling report: `crates/aetherd/src/coupling.rs`

**Batch pipeline (from 10.1):**
- `crates/aetherd/src/batch/build.rs` — reuse for re-queue JSONL generation
- `crates/aetherd/src/batch/hash.rs` — prompt hashing still applies to re-queue

**petgraph:**
- Already in workspace via `aether-graph-algo` crate
- Use `petgraph::graph::DiGraph` or `petgraph::graphmap::DiGraphMap`

---

## New config: `[continuous]`

```toml
[continuous]
enabled = false
schedule = "nightly"
staleness_half_life_days = 15
staleness_sigmoid_k = 0.3
neighbor_decay = 0.5
neighbor_cutoff = 0.1
coupling_predict_threshold = 0.85
priority_pagerank_alpha = 0.2
max_requeue_per_run = 500
auto_submit = false
requeue_pass = "triage"
```

Add `ContinuousConfig` struct in new file `crates/aether-config/src/continuous.rs`. Add `continuous: Option<ContinuousConfig>` to `AetherConfig`.

---

## Scope guard — do NOT modify

- Batch pipeline behavior from 10.1 (only call into it, don't change it)
- Existing CLI subcommands
- Existing watcher behavior
- Existing config fields

---

## End-of-stage sequence

```bash
cd /home/rephu/aether-phase10-continuous
git push -u origin feature/phase10-stage10-2-continuous-intel

# After PR merges:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase10-continuous
git branch -d feature/phase10-stage10-2-continuous-intel
```
