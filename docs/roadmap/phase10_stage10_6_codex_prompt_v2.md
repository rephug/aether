# Codex Prompt — Phase 10.6: Task Context Engine (v2)

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
- `docs/roadmap/phase_10_stage_10_6_task_context_v2.md` (the spec — read for domain context, but the implementation plan below takes precedence where they conflict)
- `docs/roadmap/phase10_stage10_6_session_context.md` (session context)
- `crates/aetherd/src/sir_context.rs` (R.1 shared export engine — `ExportDocument`, `ContextTarget`, `TargetSection`, `BudgetAllocator`, `prepare_target_section`, `allocate_export_document`, `render_export_document`)
- `crates/aetherd/src/cli.rs` (Commands enum, `ContextArgs` struct — add `--branch` flag)
- `crates/aetherd/src/main.rs` (run_subcommand dispatch)
- `crates/aether-store/src/embeddings.rs` (`search_symbols_semantic(query_embedding, provider, model, limit)` → `Vec<SemanticSearchResult>`)
- `crates/aether-store/src/symbols.rs` (`search_symbols(query, limit)` → `Vec<SymbolSearchResult>` for sparse/keyword retrieval, `get_symbol_record(id)`)
- `crates/aether-store/src/graph.rs` (`list_graph_dependency_edges()` → `Vec<GraphDependencyEdgeRecord>` for in-memory graph)
- `crates/aether-store/src/graph_cozo_compat.rs` (`list_co_change_edges_for_file()` for coupling expansion)
- `crates/aether-graph-algo/src/lib.rs` (`page_rank_sync()` — standard PageRank only, NO personalized PageRank exists yet)
- `crates/aether-infer/src/loaders.rs` (`load_embedding_provider_from_config(workspace, overrides)` → `Option<LoadedEmbeddingProvider>`)
- `crates/aether-infer/src/types.rs` (`EmbeddingProvider` trait with `embed_text(&self, text)` → `Vec<f32>`)
- `crates/aetherd/src/indexer.rs` (private `changed_paths_between_heads(workspace, old_sha, new_sha)` at line ~1723 — pattern to follow for branch diff, but do NOT import it; reimplement in task_context.rs since it's private)
- `crates/aetherd/src/continuous/monitor.rs` (pattern for loading dependency graph into petgraph, computing PageRank)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase10-task-context -b feature/phase10-stage10-6-task-context
cd /home/rephu/aether-phase10-task-context
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any are false, STOP and report:

1. Phase Repo R.1 is merged — `sir_context.rs` contains `pub enum ContextTarget { File, Symbol }`, `pub struct ExportDocument`, `pub fn run_context_command()`, `ContextArgs` in cli.rs has `--task` flag (keyword bias only).
2. Schema version is **10** (from 10.2). `SirMetaRecord` has `staleness_score: Option<f64>` and `prompt_hash: Option<String>`.
3. **`search_symbols_semantic` exists on SqliteStore** — takes `(query_embedding: &[f32], provider: &str, model: &str, limit: u32)` → `Vec<SemanticSearchResult { symbol_id, qualified_name, file_path, language, kind, semantic_score }>`.
4. **`search_symbols` exists via `SymbolCatalogStore` trait** — takes `(query: &str, limit: u32)` → `Vec<SymbolSearchResult { symbol_id, qualified_name, file_path, ... }>`. This is LIKE-based keyword matching, not FTS5.
5. **`list_graph_dependency_edges` on SqliteStore** returns all structural edges from SQLite. Use for in-memory petgraph loading. Do NOT query SurrealDB for graph data.
6. **Standard `page_rank_sync` exists** but NOT Personalized PageRank. You must implement PPR (power iteration with biased restart vector) in the new module.
7. **`gix` is a workspace dependency.** `gix::discover(workspace)` opens the repo. `repo.rev_parse_single("branch_name")` resolves branch names to commits. Follow the pattern in `indexer.rs::changed_paths_between_heads` for tree diff.
8. **Coupling is in SurrealDB only.** `CozoGraphStore::open_readonly(workspace)` → `list_co_change_edges_for_file(path, min_score)`. Handle lock failure gracefully (skip coupling, add notice).
9. **Key R.1 assembly functions are private.** You MUST change these to `pub(crate)` in `sir_context.rs`:
   - `prepare_target_section`
   - `allocate_export_document`
   - `build_project_overview`
   - `render_export_document`
   - `parse_context_format`
   - `parse_layer_selection`
   Do NOT modify their signatures or behavior. Only change `fn` → `pub(crate) fn`.
10. **Schema migration v11 needed** for `task_context_history` table (stores task descriptions + resolved symbol sets for historical precedent).
11. **Compatibility guards** in `aether-dashboard/src/state.rs` (1 call) and `aether-mcp/src/state.rs` (4 calls + 1 test) must be bumped from 10 → 11.

## IMPLEMENTATION

### Step 1: Visibility changes in sir_context.rs

Change these functions from `fn` to `pub(crate) fn` in `crates/aetherd/src/sir_context.rs`:
- `prepare_target_section`
- `allocate_export_document`
- `build_project_overview`
- `render_export_document`
- `parse_context_format`
- `parse_layer_selection`

Also add `pub(crate)` to these types if not already public:
- `PreparedTargetSection`
- `BudgetAllocator`

**Do NOT change any function signatures, logic, or existing behavior.** This is a pure visibility change. Run `cargo test -p aetherd` immediately after to confirm nothing breaks.

### Step 2: Add `--branch` to ContextArgs

In `crates/aetherd/src/cli.rs`, add to `ContextArgs`:

```rust
#[arg(
    long,
    help = "Derive task scope from branch diff against main",
    conflicts_with_all = ["targets", "symbol", "overview"]
)]
pub branch: Option<String>,
```

This means `context --branch feature/fix-auth` is a fourth targeting mode alongside file targets, --symbol, and --overview.

### Step 3: Schema migration v11

In `crates/aether-store/src/schema.rs`, add after the `if version < 10` block:

```rust
if version < 11 {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS task_context_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_description TEXT NOT NULL,
            branch_name TEXT,
            resolved_symbol_ids TEXT NOT NULL,
            resolved_file_paths TEXT NOT NULL,
            total_symbols INTEGER NOT NULL,
            budget_used INTEGER NOT NULL,
            budget_max INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_task_context_history_created
            ON task_context_history(created_at DESC);
    "#)?;
    conn.execute("PRAGMA user_version = 11", [])?;
}
```

Update compatibility guards:
- `crates/aether-dashboard/src/state.rs`: `check_compatibility("core", 10)` → `11`
- `crates/aether-mcp/src/state.rs`: all `check_compatibility("core", 10)` → `11` (4 calls + 1 test assertion)

Add a `TaskContextHistoryRecord` struct and `insert_task_context_history`/`list_recent_task_history` methods on SqliteStore.

Run `cargo test -p aether-store`, `cargo test -p aether-dashboard`, `cargo test -p aether-mcp` after.

### Step 4: Task context module

Create `crates/aetherd/src/task_context.rs` with these components:

#### 4a. Dense retrieval (embedding search)

```rust
fn dense_retrieval(
    workspace: &Path,
    store: &SqliteStore,
    task_description: &str,
    limit: u32,
) -> Result<Vec<(String, f32)>>  // (symbol_id, semantic_score)
```

1. Load embedding provider via `load_embedding_provider_from_config(workspace, EmbeddingProviderOverrides::default())`
2. If None (embeddings disabled) → return empty vec with warning
3. Create a tokio runtime, call `provider.embed_text(task_description).await`
4. Call `store.search_symbols_semantic(&embedding, provider_name, model_name, limit)`
5. Return `(symbol_id, semantic_score)` pairs

#### 4b. Sparse retrieval (keyword search)

```rust
fn sparse_retrieval(
    store: &SqliteStore,
    task_description: &str,
    limit: u32,
) -> Result<Vec<(String, usize)>>  // (symbol_id, rank)
```

1. Extract keywords from task description (split on whitespace/punctuation, filter stopwords, keep tokens ≥3 chars)
2. Call `store.search_symbols(keyword_query, limit)` for top terms
3. Return results ranked by match quality

#### 4c. Reciprocal Rank Fusion

```rust
fn reciprocal_rank_fusion(
    dense: &[(String, f32)],
    sparse: &[(String, usize)],
    k: f64,  // default 60.0
) -> Vec<(String, f64)>  // (symbol_id, rrf_score) sorted descending
```

```
RRF_score(s) = 1/(k + rank_dense(s)) + 1/(k + rank_sparse(s))
```

Symbols appearing in only one list get a single-source score. Merge and sort descending.

#### 4d. Personalized PageRank

```rust
fn personalized_pagerank(
    graph: &DiGraph<String, ()>,
    node_map: &HashMap<String, NodeIndex>,
    seed_symbol_ids: &[String],
    alpha: f64,        // restart probability, default 0.15
    iterations: usize, // default 20
) -> HashMap<String, f64>  // symbol_id → PPR score
```

**This is NOT standard PageRank.** The restart vector is biased toward seed nodes:

```
seed_vector[i] = 1/|seeds| if node i is a seed, else 0
v_{t+1} = (1 - α) · M · v_t + α · seed_vector
```

Iterate until convergence (L1 norm change < 1e-8) or max iterations. Use the petgraph `DiGraph` loaded from SQLite `symbol_edges` via `list_graph_dependency_edges()` (same pattern as `continuous/monitor.rs::load_dependency_graph`).

#### 4e. Branch diff to symbol set

```rust
fn branch_diff_to_symbols(
    workspace: &Path,
    store: &SqliteStore,
    branch_name: &str,
) -> Result<Vec<String>>  // symbol_ids in changed files
```

1. `gix::discover(workspace)` → repo
2. Resolve `main` and `branch_name` via `repo.rev_parse_single()`
3. Diff the two tree objects (follow `indexer.rs::changed_paths_between_heads` pattern)
4. Collect changed file paths
5. For each changed file: `store.list_symbols_for_file(path)` → collect symbol IDs
6. Optionally expand via coupling (SurrealDB, graceful fallback)

#### 4f. Score fusion and seed selection

```rust
fn resolve_task_symbols(
    workspace: &Path,
    store: &SqliteStore,
    task_description: &str,
    branch: Option<&str>,
    top_k_seeds: usize,  // default 20
    beta: f64,            // RRF vs PPR blend, default 0.6
) -> Result<Vec<(String, f64)>>  // (symbol_id, final_score) sorted descending
```

1. Run dense + sparse retrieval on `task_description`
2. RRF fusion → top K seed symbols
3. If `branch` provided: union branch-diff symbols into seeds
4. Load dependency graph into petgraph (from SQLite)
5. Run PPR from seed set
6. Blend: `final_score = β·RRF + (1-β)·PPR` for all symbols with nonzero score
7. Sort descending
8. Return all scored symbols (caller decides how many to use)

### Step 5: Wire task context into R.1's assembly engine

In `crates/aetherd/src/sir_context.rs`, modify `run_context_command()`:

1. If `args.branch` is `Some(branch)`:
   - Call `resolve_task_symbols(workspace, &store, task_description, Some(branch), ...)`
   - Convert top N scored symbols into `Vec<ContextTarget::Symbol { selector, file_hint }>` targets
   - Feed into the existing `prepare_target_section` → `allocate_export_document` → `render_export_document` pipeline
2. The `--task` flag (already on ContextArgs from R.1) provides the task description for ranking
3. If `--branch` without `--task`: use the branch name as the task description fallback

**This is ~20 lines of new code in `run_context_command`, not a rewrite.** The heavy lifting is in `task_context.rs`.

### Step 6: Task context history

After successful context assembly, write a row to `task_context_history`:

```rust
store.insert_task_context_history(&TaskContextHistoryRecord {
    task_description: task.to_owned(),
    branch_name: branch.map(str::to_owned),
    resolved_symbol_ids: serde_json::to_string(&symbol_ids)?,
    resolved_file_paths: serde_json::to_string(&file_paths)?,
    total_symbols: symbol_ids.len(),
    budget_used: document.budget_usage.total_used,
    budget_max: document.budget_usage.total_budget,
    created_at: unix_timestamp_secs(),
})?;
```

### Step 7: CLI additions

Add to `Commands` enum in `cli.rs`:

```rust
/// Show recent task context history
TaskHistory(TaskHistoryArgs),
/// Show task-to-symbol relevance scores without assembling context
TaskRelevance(TaskRelevanceArgs),
```

```rust
#[derive(Debug, Clone, Args)]
pub struct TaskHistoryArgs {
    #[arg(long, default_value_t = 10)]
    pub limit: usize,
}

#[derive(Debug, Clone, Args)]
pub struct TaskRelevanceArgs {
    /// Task description
    pub task: String,
    /// Branch for diff-based scoping
    #[arg(long)]
    pub branch: Option<String>,
    /// Number of top symbols to show
    #[arg(long, default_value_t = 20)]
    pub top: usize,
}
```

`task-history` prints recent task resolutions (date, task description, symbol count, budget usage).
`task-relevance` runs the ranking pipeline and prints scored symbols without assembling full context — useful for debugging and understanding what AETHER thinks is relevant.

Wire dispatch in `run_subcommand()`.

## SCOPE GUARD — Do NOT modify

- R.1's `ExportDocument` struct or renderer behavior (only change function visibility)
- Legacy `sir-context` behavior
- Existing batch/watcher/continuous behavior
- Existing dashboard pages

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aether-store -- -D warnings
cargo clippy -p aetherd -- -D warnings
cargo clippy -p aether-dashboard -- -D warnings
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-store
cargo test -p aetherd
cargo test -p aether-dashboard
cargo test -p aether-mcp
```

Verify CLI:
```bash
$CARGO_TARGET_DIR/debug/aetherd context --help
$CARGO_TARGET_DIR/debug/aetherd task-history --help
$CARGO_TARGET_DIR/debug/aetherd task-relevance --help
```

### Validation criteria

1. All tests pass, zero clippy warnings
2. Schema migration bumps to **11**, `task_context_history` table exists
3. Compatibility guards updated in dashboard and MCP state.rs (10 → 11)
4. `context --help` shows `--branch` flag
5. RRF unit tests: single-source symbols get score, dual-source symbols get higher score, output sorted descending
6. PPR unit tests: seed nodes have highest scores, scores decay with graph distance, converges within max iterations
7. Branch diff tests: changed files mapped to symbols correctly
8. `task-relevance` prints scored symbols without assembling context
9. `task-history` prints recent entries (or "no history" if empty)
10. Visibility changes in sir_context.rs don't break existing `cargo test -p aetherd`
11. Embeddings-disabled path: dense retrieval returns empty vec, sparse-only RRF still works, context assembly completes with notice

## COMMIT

```bash
git add -A
git commit -m "Phase 10.6: Task context engine — task-driven context assembly

Task-to-symbol relevance ranking:
- Dense retrieval via SIR embedding similarity (LanceDB ANN)
- Sparse retrieval via keyword matching on qualified names
- Reciprocal Rank Fusion (k=60) to merge dense + sparse signals
- Personalized PageRank expansion from top-K seed symbols on SQLite dependency graph
- Score blending: β·RRF + (1-β)·PPR for final symbol ranking

Branch diff integration:
- context --branch derives task scope from gix diff against main
- Changed files → symbol mapping → coupling expansion → PPR seeds
- Graceful fallback when SurrealDB coupling unavailable

Assembly:
- Task-ranked symbols feed into R.1's shared ExportDocument engine
- Same budget allocator, same renderers, same output formats
- Task description biases within-tier ordering

Task history:
- task_context_history SQLite table (schema v11)
- aetherd task-history shows recent task resolutions
- aetherd task-relevance shows ranked symbols without full assembly

Schema:
- Migration v11: task_context_history table
- Compatibility guards updated in dashboard and MCP"
```

**PR title:** Phase 10.6: Task context engine — task-driven context assembly
**PR body:** Adds task-driven context assembly to the existing `aetherd context` command. Given a task description or branch name, ranks all symbols by relevance using RRF fusion of dense (embedding) + sparse (keyword) retrieval, then expands structurally via Personalized PageRank. Results feed into R.1's shared assembly engine for budgeted context output. New `task-history` and `task-relevance` CLI commands for debugging and history tracking. Schema migration v11.

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase10-stage10-6-task-context
```
