# Codex Prompt — Phase 10.2: Continuous Intelligence (v2)

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
- `docs/roadmap/phase_10_stage_10_2_continuous_intelligence_v2.md` (the spec)
- `docs/roadmap/phase10_stage10_2_session_context.md` (session context)
- `crates/aetherd/src/batch/build.rs` (reuse for re-queue JSONL — `build_pass_jsonl()`)
- `crates/aetherd/src/batch/hash.rs` (prompt hashing)
- `crates/aetherd/src/batch/ingest.rs` (contains `semantic_delta()` cosine distance — REUSE, do not duplicate)
- `crates/aetherd/src/batch/mod.rs` (BatchRuntimeConfig, PassConfig, resolve helpers)
- `crates/aether-health/src/git_signals.rs` (`compute_file_git_stats()` — git churn)
- `crates/aether-graph-algo/src/lib.rs` (`page_rank_sync()` — PageRank computation)
- `crates/aether-config/src/root.rs` (add ContinuousConfig here)
- `crates/aether-store/src/schema.rs` (migrations — current version is **9**, next is 10)
- `crates/aether-store/src/sir_meta.rs` (SirMetaRecord — add `staleness_score`)
- `crates/aether-store/src/graph.rs` (`list_graph_dependency_edges()` on SqliteStore — structural edges)
- `crates/aether-store/src/fingerprint_history.rs` (`list_sir_fingerprint_history()` — for volatility)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase10-continuous -b feature/phase10-stage10-2-continuous-intel
cd /home/rephu/aether-phase10-continuous
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any are false, STOP and report:

1. Stage 10.1 is merged — `sir_fingerprint_history` table exists, `batch build`/`ingest` work, schema version is **9**
2. `petgraph` is in workspace dependencies (via `aether-graph-algo`)
3. **The SIR table is named `sir` (NOT `sir_meta`).** Primary key is `id` (NOT `symbol_id`). The Rust struct is `SirMetaRecord` in `sir_meta.rs`. It already has `prompt_hash: Option<String>` from 10.1.
4. **Structural edges are in SQLite.** The `symbol_edges` table stores CALLS, DEPENDS_ON, TYPE_REF, IMPLEMENTS edges. Access via `SqliteStore::list_graph_dependency_edges()` in `crates/aether-store/src/graph.rs`. This returns `Vec<GraphDependencyEdgeRecord>` with `source_symbol_id`, `target_symbol_id`, `edge_kind`. **Use SQLite for all edge queries — avoids SurrealKV lock contention.**
5. **Coupling data is in SurrealDB only (NOT SQLite).** There is NO `coupling_pairs` SQLite table. Access coupling via `graph_store.list_co_change_edges_for_file(file_path, min_fused_score)` and `graph_store.list_top_co_change_edges(limit)` through the `graph_cozo_compat.rs` shim. For nightly cron (`continuous run-once`), SurrealDB is accessible because no daemon holds the lock.
6. **`git_churn_30d` is NOT pre-computed globally.** Compute per-file dynamically: `aether_core::GitContext::open(workspace)` then `aether_health::git_signals::compute_file_git_stats(&git, &path)` for each unique file. Returns `FileGitStats { commits_30d, commits_90d, author_count, blame_age_std_dev }`. Map `commits_30d` to all symbols in that file.
7. **PageRank:** `aether_graph_algo::page_rank_sync(edges, damping, iterations)` takes `&[GraphAlgorithmEdge]` and returns `Vec<(String, f64)>`. The `GraphAlgorithmEdge` has `source: String, target: String`. You need to convert `GraphDependencyEdgeRecord` to `GraphAlgorithmEdge`.
8. **Cosine distance already exists** in `crates/aetherd/src/batch/ingest.rs` as the private function `semantic_delta()`. Extract this to a shared location (e.g., `crates/aetherd/src/batch/hash.rs` or a new `crates/aetherd/src/continuous/math.rs`) rather than duplicating it. Also `watcher_semantic_delta()` in `indexer.rs` is an identical copy — all three should collapse to one function.
9. **Schema compatibility guards exist in two state.rs files.** These assert `store.check_compatibility("core", N)` where N is the max schema version the binary understands. After bumping schema to 10, you MUST also update:
   - `crates/aether-dashboard/src/state.rs` — 1 call (currently `check_compatibility("core", 9)`)
   - `crates/aether-mcp/src/state.rs` — 4 calls + 1 test assertion (currently `check_compatibility("core", 9)`)
   All must change from 9 to **10**. Missing this causes runtime failures in downstream crates.
10. **`sir_fingerprint_history` access** — `SqliteStore::list_sir_fingerprint_history(symbol_id)` returns `Vec<SirFingerprintHistoryRecord>` with `delta_sem: Option<f64>`, `timestamp`, `trigger`, and change flags. Already implemented in `crates/aether-store/src/fingerprint_history.rs`.

## IMPLEMENTATION

### Step 1: ContinuousConfig

Create `crates/aether-config/src/continuous.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuousConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default = "default_schedule")] pub schedule: String,
    #[serde(default = "default_half_life")] pub staleness_half_life_days: f64,
    #[serde(default = "default_sigmoid_k")] pub staleness_sigmoid_k: f64,
    #[serde(default = "default_neighbor_decay")] pub neighbor_decay: f64,
    #[serde(default = "default_neighbor_cutoff")] pub neighbor_cutoff: f64,
    #[serde(default = "default_coupling_threshold")] pub coupling_predict_threshold: f64,
    #[serde(default = "default_pr_alpha")] pub priority_pagerank_alpha: f64,
    #[serde(default = "default_max_requeue")] pub max_requeue_per_run: usize,
    #[serde(default)] pub auto_submit: bool,
    #[serde(default = "default_requeue_pass")] pub requeue_pass: String,
}
```

Defaults: `schedule = "nightly"`, `staleness_half_life_days = 15.0`, `staleness_sigmoid_k = 0.3`, `neighbor_decay = 0.5`, `neighbor_cutoff = 0.1`, `coupling_predict_threshold = 0.85`, `priority_pagerank_alpha = 0.2`, `max_requeue_per_run = 500`, `requeue_pass = "triage"`.

Add `continuous: Option<ContinuousConfig>` to `AetherConfig` in `root.rs`.
Register module in `lib.rs`. Unit tests: empty TOML parses, full `[continuous]` section parses.

### Step 1b: Schema migration for staleness_score

In `crates/aether-store/src/schema.rs`, add a new migration block **after the existing `if version < 9` block**:

```rust
if version < 10 {
    ensure_sir_column(conn, "staleness_score", "REAL")?;
    conn.execute("PRAGMA user_version = 10", [])?;
}
```

Update `SirMetaRecord` in `sir_meta.rs`:
- Add `pub staleness_score: Option<f64>` field
- Update the INSERT/UPSERT SQL to include `staleness_score`
- Update the SELECT queries to read `staleness_score`
- Update all existing `SirMetaRecord` construction sites (search for `SirMetaRecord {` across the workspace)

**CRITICAL: Update schema compatibility guards:**
- `crates/aether-dashboard/src/state.rs`: change `check_compatibility("core", 9)` → `check_compatibility("core", 10)` (1 call)
- `crates/aether-mcp/src/state.rs`: change all `check_compatibility("core", 9)` → `check_compatibility("core", 10)` (4 calls + 1 test assertion)

Run `cargo test -p aether-dashboard`, `cargo test -p aether-mcp`, `cargo test -p aether-store` after this step to catch any missed construction sites or compatibility issues early.

### Step 2: Staleness scoring module

Create `crates/aetherd/src/continuous/` module:
- `mod.rs` — re-exports
- `staleness.rs` — scoring functions
- `monitor.rs` — drift monitor orchestrator
- `priority.rs` — priority ranking
- `math.rs` — shared math utilities (extract cosine distance here)

**staleness.rs:**

```rust
/// Logistic sigmoid time decay
pub fn time_staleness(days_since: f64, half_life: f64, k: f64) -> f64 {
    1.0 / (1.0 + (-k * (days_since - half_life)).exp())
}

/// Volatility-adjusted effective age
pub fn effective_age(days_since: f64, git_churn_30d: f64) -> f64 {
    days_since * (1.0 + (1.0 + git_churn_30d).log2())
}

/// Noisy-OR combination of soft signals
pub fn noisy_or(s_time: f64, s_neighbor: f64) -> f64 {
    1.0 - (1.0 - s_time) * (1.0 - s_neighbor)
}

/// Full staleness score
pub fn compute_staleness(
    source_changed: bool,
    model_deprecated: bool,
    s_time: f64,
    s_neighbor: f64,
) -> f64 {
    let s_source = if source_changed { 1.0 } else { 0.0 };
    let s_model = if model_deprecated { 1.0 } else { 0.0 };
    let soft = noisy_or(s_time, s_neighbor);
    s_source.max(s_model).max(soft)
}
```

Unit tests: verify sigmoid curve shape (0 at t=0 when half_life>>0, ~0.5 at t=half_life, approaches 1.0 at t>>half_life), verify hard gates override soft signals, verify noisy-or bounds [0,1], verify effective_age increases with churn.

### Step 3: Shared cosine distance

**Extract** the cosine distance function from `crates/aetherd/src/batch/ingest.rs::semantic_delta()` into `crates/aetherd/src/continuous/math.rs` (or `crates/aetherd/src/batch/hash.rs`). Make it `pub(crate)`.

Then update:
- `batch/ingest.rs` — import from new location, delete local copy
- `indexer.rs` — import from new location, delete `watcher_semantic_delta()`

Signature:
```rust
pub(crate) fn cosine_distance_from_embeddings(
    previous: Option<&SymbolEmbeddingRecord>,
    current: Option<&SymbolEmbeddingRecord>,
) -> Option<f64>
```

This is a refactor with zero behavior change — run `cargo test -p aetherd` after to confirm.

### Step 4: In-memory graph loading

**Use SQLite `symbol_edges` table via `SqliteStore::list_graph_dependency_edges()`**, NOT SurrealDB.

```rust
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use aether_store::SqliteStore;

pub fn load_dependency_graph(
    store: &SqliteStore,
) -> Result<(DiGraph<String, ()>, HashMap<String, NodeIndex>)> {
    let edges = store.list_graph_dependency_edges()?;
    let mut graph = DiGraph::new();
    let mut node_map = HashMap::new();
    for edge in &edges {
        let source_idx = *node_map.entry(edge.source_symbol_id.clone())
            .or_insert_with(|| graph.add_node(edge.source_symbol_id.clone()));
        let target_idx = *node_map.entry(edge.target_symbol_id.clone())
            .or_insert_with(|| graph.add_node(edge.target_symbol_id.clone()));
        graph.add_edge(source_idx, target_idx, ());
    }
    Ok((graph, node_map))
}
```

Traverse **reverse** edges (dependents) for the discounted BFS:

```rust
pub fn propagate_neighbor_staleness(
    graph: &DiGraph<String, ()>,
    node_map: &HashMap<String, NodeIndex>,
    seed_staleness: &HashMap<String, f64>,
    delta_sem: &HashMap<String, f64>,
    gamma: f64,
    cutoff: f64,
) -> HashMap<String, f64> {
    // BFS from each seed, propagate S × γ × Δ_sem along reverse edges
    // Use petgraph::Direction::Incoming to walk reverse edges
    // Prune when S_indirect < cutoff
    // Return induced staleness per symbol
}
```

### Step 5: Predictive staleness from coupling

**Coupling data is in SurrealDB.** For `continuous run-once` (nightly cron with no daemon running), SurrealDB is accessible.

```rust
pub fn coupling_predict(
    recently_edited: &HashSet<String>,  // file paths edited since last run
    graph_store: &dyn GraphStore,       // SurrealDB access for coupling data
    threshold: f64,
) -> HashMap<String, f64> {
    // For each edited file A:
    //   edges = graph_store.list_co_change_edges_for_file(A, threshold)
    //   For each edge where fused_score > threshold:
    //     bump the other file's symbols: coupling * 0.5
    // Return symbol_id → soft staleness bump
}
```

Map file-level bumps to symbol-level by looking up which symbols live in each file (query `symbols` SQLite table grouped by `file_path`).

**IMPORTANT:** If SurrealDB is unavailable (e.g., lock contention because daemon is running), log a warning and skip coupling prediction entirely. Do not panic or fail the whole run.

### Step 6: Priority ranking

```rust
pub fn compute_priority(s_total: f64, pagerank: f64, pr_max: f64, alpha: f64) -> f64 {
    let pr_normalized = if pr_max > 0.0 {
        (1.0 + pagerank).log10() / (1.0 + pr_max).log10()
    } else {
        0.0
    };
    s_total + alpha * pr_normalized
}
```

### Step 7: Monitor orchestrator

**monitor.rs:**

1. Load SQLite `symbol_edges` into in-memory `petgraph::DiGraph` via `store.list_graph_dependency_edges()` (NOT SurrealDB — avoids lock contention)
2. Load all symbols + sir records (from `sir` table) including `generation_pass`, `model`, `updated_at`, `prompt_hash`
3. Compute PageRank: convert `GraphDependencyEdgeRecord` → `GraphAlgorithmEdge`, call `page_rank_sync(edges, 0.85, 25)`
4. **Compute git churn per file dynamically:** `GitContext::open(workspace)`, group all candidate symbols by `file_path`, call `compute_file_git_stats(&git, &path)` for each unique file. Map resulting `commits_30d` to all symbols in that file.
5. Load coupling data from SurrealDB (safe for nightly cron — no daemon holds lock). Graceful fallback if unavailable.
6. Load recent `sir_fingerprint_history` entries for volatility detection: `store.list_sir_fingerprint_history(symbol_id)` for each candidate
7. Compute S_total for all symbols using the Noisy-OR formula
8. Apply predictive coupling bumps
9. Apply volatility bumps (≥3 events with Δ_sem > 0.2 in 30 days → multiply time staleness by 1.5)
10. Compute priority = S_total + α × log-dampened PageRank
11. Sort by priority descending, take top `max_requeue_per_run`
12. Call 10.1's `batch build` to generate JSONL (prompt hashing still applies — skip symbols whose prompts haven't changed)
13. Write computed `staleness_score` back to the `sir` table for each processed symbol
14. If `auto_submit`: call `batch run` machinery

### Step 8: CLI wiring

Add to `Commands` enum in `cli.rs`:
```rust
/// Continuous intelligence operations
Continuous(ContinuousArgs),
```

Subcommands:
```rust
pub enum ContinuousCommand {
    RunOnce(ContinuousRunOnceArgs),
    Status,
}
```

`run-once` executes one drift monitor cycle. `status` prints summary stats (total symbols, stale count by tier, most stale symbol, last run timestamp).

Wire dispatch in `run_subcommand()` in `main.rs`.

### Step 8b: Background tokio task

In the daemon startup path (when running as persistent service, not CLI subcommand), spawn a background task if `[continuous] enabled = true`:

```rust
if let Some(continuous) = &config.continuous {
    if continuous.enabled {
        let config_clone = config.clone();
        let workspace_clone = workspace.to_path_buf();
        tokio::spawn(async move {
            loop {
                let interval = parse_schedule(&config_clone.continuous.as_ref().unwrap().schedule);
                tokio::time::sleep(interval).await;
                if let Err(e) = run_drift_monitor(&workspace_clone, &config_clone) {
                    tracing::error!("Drift monitor cycle failed: {e}");
                }
            }
        });
    }
}
```

`parse_schedule()` converts `"nightly"` → 24h, `"hourly"` → 1h. MVP: support only these two — defer cron expression parsing.

### Step 9: Post-build trigger

In `indexer.rs` or a new file: when `[watcher] trigger_on_build_success = true`, watch `target/` for changes to build artifacts. On detection, identify source files with `mtime` newer than build start time and queue them for re-indexing.

This is approximate and low-priority — implement a minimal stub that logs "build trigger detected" and re-queues recently modified `.rs` files. Full implementation can be refined later.

## SCOPE GUARD — Do NOT modify

- Batch pipeline behavior from 10.1 (only call into it, don't change it beyond extracting the shared cosine distance)
- Existing CLI subcommands
- Existing watcher behavior
- Existing config fields (only ADD new optional fields)

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aether-config -- -D warnings
cargo clippy -p aether-store -- -D warnings
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-dashboard -- -D warnings
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-config
cargo test -p aether-store
cargo test -p aether-dashboard
cargo test -p aether-mcp
cargo test -p aetherd
```

Verify CLI:
```bash
./target/debug/aetherd continuous --help
./target/debug/aetherd continuous run-once --help
./target/debug/aetherd continuous status --help
```

### Validation criteria

1. All tests pass, zero clippy warnings
2. `continuous --help` shows subcommands
3. Schema migration: version bumps from 9 to **10**, `sir` table has `staleness_score` column
4. **Compatibility guards updated**: `aether-dashboard` and `aether-mcp` state.rs files accept schema version 10
5. Staleness unit tests: sigmoid shape, hard gate override, noisy-or bounds, effective_age increases with churn
6. Priority unit tests: stale leaf outranks barely-stale hub, PageRank breaks ties between equal staleness
7. Config tests: empty TOML parses, full `[continuous]` section parses
8. Cosine distance deduplicated: no more than one copy of the cosine distance computation in the codebase
9. Monitor produces re-queue JSONL via 10.1 `batch build` with prompt hash skip logic
10. `staleness_score` written back to `sir` table after monitor run
11. `cargo test -p aether-dashboard`, `cargo test -p aether-mcp` pass (compatibility guard validation)

## COMMIT

```bash
git add -A
git commit -m "Phase 10.2: Continuous intelligence — drift monitor + staleness scoring

Staleness scoring:
- Noisy-OR formula with hard gates for source changes and model deprecation
- Logistic sigmoid time decay with configurable half-life (default 15 days)
- Cold-start volatility prior from git churn (effective_age adjustment)
- Semantic-gated neighbor propagation (Δ_sem × γ decay, BFS with cutoff)
- Predictive staleness from temporal coupling matrix (SurrealDB co-change edges)

Priority ranking:
- S_total + α × log-dampened PageRank as tiebreaker
- Staleness dominates, PageRank breaks ties among equally stale symbols

Drift monitor:
- In-memory petgraph DiGraph loaded from SQLite symbol_edges (no SurrealKV lock)
- Reads sir_fingerprint_history for volatility detection (≥3 high Δ_sem in 30d)
- Writes re-queue JSONL via 10.1 batch build (with prompt hash skip logic)
- aetherd continuous run-once for cron/nightly use on netcup servers
- aetherd continuous status for summary stats
- Background tokio task when [continuous] enabled = true

Schema:
- Migration v10: staleness_score column on sir table
- Compatibility guards updated in dashboard and MCP state.rs

Cleanup:
- Deduplicated cosine distance (was in batch/ingest.rs + indexer.rs, now shared)

Post-build trigger:
- Minimal stub for [watcher] trigger_on_build_success"
```

**PR title:** Phase 10.2: Continuous intelligence — drift monitor + staleness scoring
**PR body:** Adds Noisy-OR staleness scoring with hard gates, logistic time decay, semantic-gated neighbor propagation, and predictive coupling. Drift monitor runs as `aetherd continuous run-once` (for nightly cron on netcup) or as a background tokio task. In-memory petgraph from SQLite edges avoids SurrealKV lock contention. Reads 10.1 fingerprint history for volatility detection. Schema migration v10.

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase10-stage10-2-continuous-intel
```
