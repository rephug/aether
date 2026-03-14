# Codex Prompt — Phase 10.2: Continuous Intelligence

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
- `crates/aetherd/src/batch/build.rs` (reuse for re-queue JSONL)
- `crates/aetherd/src/batch/hash.rs` (prompt hashing)
- `crates/aetherd/src/health_score.rs` (git churn data access pattern)
- `crates/aether-config/src/root.rs` (add ContinuousConfig here)
- `crates/aether-store/src/` (embedding read functions)

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

1. Stage 10.1 is merged — `sir_fingerprint_history` table exists, `batch build`/`ingest` work
2. `petgraph` is in workspace dependencies (via `aether-graph-algo`)
3. **The SIR table is named `sir` (NOT `sir_meta`).** Primary key is `id` (NOT `symbol_id`). The Rust struct is `SirMetaRecord` in `sir_meta.rs`.
4. **Coupling data lives in SurrealDB, NOT SQLite.** Access via `graph_store.list_co_change_edges_for_file()` or `list_top_co_change_edges()`. For the nightly cron (no daemon running), SurrealDB is accessible. There is NO `coupling_pairs` SQLite table.
5. **Structural edges ARE in SQLite.** The `symbol_edges` table stores CALLS, DEPENDS_ON, TYPE_REF, IMPLEMENTS edges. Access via `store.get_callers()` and `store.get_dependencies()` on the SQLite store. This avoids SurrealKV lock contention — use SQLite for all edge queries in the drift monitor.
6. Embedding read function — find how to load a symbol's embedding vector from LanceDB or SQLite vector store
7. **`git_churn_30d` is NOT pre-computed globally.** It is computed per-file dynamically via `aether_health::git_signals::compute_file_git_stats()`. You must instantiate `aether_core::git::GitContext`, group symbols by file_path, and call `compute_file_git_stats(&git, path)` for each file. Map the resulting `commits_30d` to all symbols in that file.
8. PageRank is computed somewhere — find where (likely `aether-analysis` or health pipeline)

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

### Step 1b: Schema migration for staleness_score

In `crates/aether-store/src/schema.rs`, add (in the same version block as 10.1 if not yet merged, or in a new `if version < 9` block):

```rust
ensure_sir_column(conn, "staleness_score", "REAL")?;
```

Also update `SirMetaRecord` in `sir_meta.rs` to include `pub staleness_score: Option<f64>` and update the relevant SQL queries.

Add `continuous: Option<ContinuousConfig>` to `AetherConfig` in `root.rs`.

### Step 2: Staleness scoring module

Create `crates/aetherd/src/continuous/` module:
- `mod.rs` — re-exports
- `staleness.rs` — scoring functions
- `monitor.rs` — drift monitor orchestrator
- `priority.rs` — priority ranking

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

Unit tests: verify sigmoid curve shape, verify hard gates override soft signals, verify noisy-or bounds.

### Step 3: Semantic gate (Δ_sem)

Implement cosine distance computation:

```rust
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 { return 1.0; }
    1.0 - (dot / (norm_a * norm_b))
}
```

To get old vs new embeddings: check if `previous_embedding_hash` exists in the `sir` table. Alternatively, compute Δ_sem at SIR regeneration time (when both old and new embeddings are in hand — fetch OLD embedding BEFORE overwriting) and store the result in `sir_fingerprint_history.delta_sem` — this is already the plan from 10.1.

For the drift monitor: READ `delta_sem` from `sir_fingerprint_history` for recently regenerated neighbors. Do NOT recompute embeddings during the monitor run.

### Step 4: In-memory graph loading

**CRITICAL: Use SQLite `symbol_edges` table, NOT SurrealDB.** SurrealKV exclusive lock means querying SurrealDB while the daemon runs would crash. The `symbol_edges` table in SQLite has all structural edges (CALLS, DEPENDS_ON, TYPE_REF, IMPLEMENTS) and is lock-free.

```rust
use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;

pub fn load_dependency_graph(store: &SqliteStore) -> (DiGraph<String, ()>, HashMap<String, NodeIndex>) {
    // Query all edges from SQLite symbol_edges table
    // Use store.get_callers() and store.get_dependencies() patterns
    // Or direct SQL: SELECT source_id, target_id FROM symbol_edges
    // Build DiGraph with symbol_id as node weight
    // Return graph + symbol_id→NodeIndex map
}
```

Traverse reverse edges (dependents) for the discounted BFS:

```rust
pub fn propagate_neighbor_staleness(
    graph: &DiGraph<String, ()>,
    node_map: &HashMap<String, NodeIndex>,
    seed_staleness: &HashMap<String, f64>,  // symbols with known S > 0
    delta_sem: &HashMap<String, f64>,        // recent Δ_sem per symbol
    gamma: f64,                               // decay factor
    cutoff: f64,                              // pruning threshold
) -> HashMap<String, f64> {
    // BFS from each seed, propagate S × γ × Δ_sem along reverse edges
    // Prune when S_indirect < cutoff
    // Return induced staleness per symbol
}
```

### Step 5: Predictive staleness from coupling

**NOTE: Coupling data is in SurrealDB, accessed via `graph_store.list_co_change_edges_for_file()`.** For the nightly cron (no daemon running), SurrealDB is accessible since nothing else holds the lock.

```rust
pub fn coupling_predict(
    recently_edited: &HashSet<String>,  // file paths edited since last run
    graph_store: &SurrealGraphStore,    // SurrealDB access for coupling data
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

1. Load SQLite `symbol_edges` into in-memory `petgraph::DiGraph` (NOT SurrealDB — avoids lock contention)
2. Load all symbols + sir records (from `sir` table — NOT `sir_meta`) including `generation_pass`, `model`, `updated_at`, `prompt_hash`
3. Load PageRank scores (find existing computation in `aether-analysis`)
4. **Compute git churn per file dynamically:** Instantiate `aether_core::git::GitContext`, group all candidate symbols by `file_path`, call `aether_health::git_signals::compute_file_git_stats(&git, path)` for each unique file. Map resulting `commits_30d` to all symbols in that file. There is NO pre-computed global churn table.
5. Load coupling data from SurrealDB (safe for nightly cron — no daemon holds lock): `graph_store.list_top_co_change_edges(limit)`
6. Load recent `sir_fingerprint_history` entries (for Δ_sem + volatility)
7. Compute S_total for all symbols using the Noisy-OR formula
8. Apply predictive coupling bumps
9. Apply volatility bumps (≥3 events with Δ_sem > 0.2 in 30 days)
10. Compute priority = S_total + α × log-dampened PageRank
11. Sort by priority descending, take top `max_requeue_per_run`
12. Call 10.1's `batch build` to generate JSONL (prompt hashing still applies)
13. Write fingerprint history rows with `trigger = "drift_monitor"`
14. Write computed `staleness_score` back to the `sir` table for each symbol (for dashboard/MCP queries)
15. If `auto_submit`: call `batch run` machinery

### Step 8: CLI wiring

Add to `Commands` enum:
```rust
/// Continuous intelligence operations
Continuous(ContinuousArgs),
```

Subcommands:
```rust
pub enum ContinuousCommand {
    RunOnce(ContinuousRunOnceArgs),
    Status,  // show current staleness stats
}
```

`run-once` executes one drift monitor cycle. `status` prints summary stats (total symbols, stale count, most stale symbol, last run timestamp).

### Step 8b: Background tokio task (CRITICAL — do not skip)

The spec mandates a background drift monitor as a tokio task. Wire this in `aetherd/src/main.rs` (or `indexer.rs`) alongside other background tasks:

```rust
// In the daemon startup path (when running as persistent service, not CLI subcommand):
if let Some(continuous) = &config.continuous {
    if continuous.enabled {
        let config_clone = config.clone();
        let workspace_clone = workspace.to_path_buf();
        tokio::spawn(async move {
            loop {
                let interval = parse_schedule(&config_clone.continuous.as_ref().unwrap().schedule);
                tokio::time::sleep(interval).await;
                if let Err(e) = run_drift_monitor(&workspace_clone, &config_clone).await {
                    tracing::error!("Drift monitor failed: {e}");
                }
            }
        });
    }
}
```

The `parse_schedule()` function converts "nightly" → 24h, "hourly" → 1h. For MVP, support "nightly" and "hourly" only — defer cron expression parsing.

### Step 9: Post-build trigger

In `indexer.rs` or a new file: when `trigger_on_build_success = true`, watch the `target/` directory for changes to build artifacts. On detection, identify source files that were part of the build and queue them for re-indexing via the watcher model.

This can be approximate — detect which `.rs` files have `mtime` newer than the build start time.

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aether-config -- -D warnings
cargo clippy -p aetherd -- -D warnings
cargo test -p aether-config
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
3. Staleness unit tests: sigmoid shape, hard gate override, noisy-or bounds
4. Priority unit tests: stale leaf outranks barely-stale hub
5. Config tests: empty TOML parses, full `[continuous]` section parses

## COMMIT

```bash
git add -A
git commit -m "Phase 10.2: Continuous intelligence — drift monitor + staleness scoring

Staleness scoring:
- Noisy-OR formula with hard gates for source changes and model deprecation
- Logistic sigmoid time decay with configurable half-life
- Cold-start volatility prior from git churn data
- Semantic-gated neighbor propagation (Δ_sem × γ decay, BFS with cutoff)
- Predictive staleness from temporal coupling matrix

Priority ranking:
- S_total + α × log-dampened PageRank as tiebreaker
- Staleness dominates, PageRank breaks ties

Drift monitor:
- In-memory petgraph for fast graph traversal
- Reads sir_fingerprint_history for volatility detection
- Writes re-queue JSONL via 10.1 batch build (with prompt hashing)
- aetherd continuous run-once for cron/nightly use
- aetherd continuous status for summary stats

Post-build trigger:
- Implements [watcher] trigger_on_build_success stub from 10.1"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase10-stage10-2-continuous-intel
```
