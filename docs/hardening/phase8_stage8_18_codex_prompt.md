# Codex Prompt — Phase 8.18: Fix Boundary Leaker via Global Pipeline

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are modifying 4 files across 3 crates. Read these files before
writing any code:

- `docs/roadmap/phase_8_stage_8_18_boundary_leaker_fix.md` (the spec)
- `docs/hardening/phase8_stage8_18_session_context.md` (session context)
- `crates/aether-store/src/graph_surreal.rs` (new filtered Louvain method)
- `crates/aether-store/src/graph_cozo_compat.rs` (compat wrapper)
- `crates/aether-analysis/src/drift.rs` (update compute_boundary_records)
- `crates/aetherd/src/health_score.rs` (update load_semantic_input)
- `crates/aether-graph-algo/src/lib.rs` (READ ONLY — verify louvain_with_resolution_sync exists)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-boundary-leaker -b feature/phase8-stage8-18-boundary-leaker-fix
cd /home/rephu/aether-phase8-boundary-leaker
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any is false, STOP:

1. `louvain_with_resolution_sync(edges: &[GraphAlgorithmEdge], resolution: f64)`
   exists in `crates/aether-graph-algo/src/lib.rs` and is `pub`
2. `list_louvain_communities()` in `graph_surreal.rs` calls `louvain_sync`
   which is `louvain_with_resolution_sync(edges, 1.0)`
3. `graph_surreal.rs` imports `louvain_sync` from `aether_graph_algo` but
   does NOT import `louvain_with_resolution_sync`
4. `to_algo_edges()` converts `Vec<DependencyEdgeRow>` to
   `Vec<GraphAlgorithmEdge>` in `graph_surreal.rs`
5. SurrealDB symbol table has fields: `symbol_id`, `qualified_name`,
   `file_path` (check the schema definition in `graph_surreal.rs`)
6. `compute_boundary_records` in `drift.rs` has access to both
   `store: &SqliteStore` and `cozo: &CozoGraphStore`
7. `load_semantic_input` in `health_score.rs` has access to SqliteStore
   and creates a tokio runtime for async calls
8. `CozoGraphStore` in `graph_cozo_compat.rs` wraps `SurrealGraphStore`
   methods using `block_on_runtime`

## IMPLEMENTATION

### Step 1: graph_surreal.rs — Add filtered Louvain method

Add `louvain_with_resolution_sync` to the import from `aether_graph_algo`:

```rust
use aether_graph_algo::{
    GraphAlgorithmEdge, connected_components_sync, louvain_sync,
    louvain_with_resolution_sync, page_rank_sync,
    strongly_connected_components_sync,
};
```

Add a helper to query all symbol IDs with their test-relevant fields:

```rust
pub async fn list_symbol_ids_with_paths(&self) -> Result<Vec<(String, String, String)>, StoreError> {
    // Returns (symbol_id, qualified_name, file_path) for all symbols
    let mut response = self.db
        .query("SELECT VALUE { id: symbol_id, qn: qualified_name, fp: file_path } FROM symbol;")
        .await
        .map_err(|err| StoreError::Graph(format!("symbol path query failed: {err}")))?;
    let rows: Vec<serde_json::Value> = response.take(0)
        .map_err(|err| StoreError::Graph(format!("symbol path decode failed: {err}")))?;
    let mut results = Vec::new();
    for row in rows {
        let id = row.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let qn = row.get("qn").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        let fp = row.get("fp").and_then(|v| v.as_str()).unwrap_or_default().to_owned();
        if !id.is_empty() {
            results.push((id, qn, fp));
        }
    }
    Ok(results)
}
```

Add the filtered Louvain method:

```rust
pub async fn list_louvain_communities_filtered(
    &self,
    resolution: f64,
    exclude_symbol_ids: &HashSet<String>,
) -> Result<Vec<(String, i64)>, StoreError> {
    let edges = self.list_dependency_edges_raw().await?;
    if edges.is_empty() {
        return Ok(Vec::new());
    }
    let filtered_edges: Vec<DependencyEdgeRow> = edges
        .into_iter()
        .filter(|edge| {
            !exclude_symbol_ids.contains(&edge.source_id)
                && !exclude_symbol_ids.contains(&edge.target_id)
        })
        .collect();
    if filtered_edges.is_empty() {
        return Ok(Vec::new());
    }
    let algo_edges = to_algo_edges(filtered_edges);
    let assignments = tokio::task::spawn_blocking(move || {
        louvain_with_resolution_sync(&algo_edges, resolution)
    })
    .await
    .map_err(|err| StoreError::Graph(format!("spawn_blocking louvain filtered failed: {err}")))?;
    let mut records = assignments
        .into_iter()
        .map(|(node, community)| (node, community as i64))
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}
```

**IMPORTANT:** Check that `DependencyEdgeRow` has fields named
`source_id` and `target_id`. If the field names are different (e.g.
`source_symbol_id` / `target_symbol_id`), use the actual field names.
Inspect the struct definition before writing the filter.

### Step 2: graph_cozo_compat.rs — Add compat wrapper

```rust
pub fn list_louvain_communities_filtered(
    &self,
    resolution: f64,
    exclude_symbol_ids: &HashSet<String>,
) -> Result<Vec<(String, i64)>, StoreError> {
    Self::block_on_runtime(
        self.inner.list_louvain_communities_filtered(resolution, exclude_symbol_ids)
    )
}

pub fn list_symbol_ids_with_paths(&self) -> Result<Vec<(String, String, String)>, StoreError> {
    Self::block_on_runtime(self.inner.list_symbol_ids_with_paths())
}
```

Add `HashSet` to the imports if not already present.

### Step 3: drift.rs — Update compute_boundary_records

Add a helper function (file-level or inside the impl):

```rust
fn is_test_symbol_heuristic(qualified_name: &str, file_path: &str) -> bool {
    let leaf = qualified_name.rsplit("::").next().unwrap_or(qualified_name);
    if leaf.to_ascii_lowercase().starts_with("test_") {
        return true;
    }
    let path = file_path.to_ascii_lowercase();
    path.starts_with("tests/") || path.contains("/tests/")
}
```

In `compute_boundary_records`:
- Query test symbol IDs using `cozo.list_symbol_ids_with_paths()`
- Build `HashSet<String>` of test symbol IDs
- Replace `cozo.list_louvain_communities()` with
  `cozo.list_louvain_communities_filtered(0.5, &test_symbol_ids)`

The resolution 0.5 matches the file-scoped planner's default γ
(`config.planner.community_resolution`). If you can thread config
through to this method, use `config.planner.community_resolution`.
If threading config is complex, hardcode 0.5 for now.

### Step 4: health_score.rs — Update load_semantic_input

Change signature:
```rust
fn load_semantic_input(workspace: &Path, config: &AetherConfig) -> Result<Option<SemanticInput>>
```

Update the call site in `compute_current_report` to pass `config`.

Inside `load_semantic_input`, after the centrality call:
- Open SurrealGraphStore (the centrality call already dropped its
  connection, so no lock contention)
- Query symbol IDs with paths
- Build test symbol ID set using the same heuristic
- Call `list_louvain_communities_filtered(resolution, &test_ids)`
  where resolution = `config.planner.community_resolution`
- Build `community_by_symbol: HashMap<String, i64>` from the result
- Remove the `store.list_latest_community_snapshot()` lookup
- The rest of the function (per-file community_count computation from
  the map) stays exactly as it was before

If opening SurrealDB or querying fails, fall back to an empty
`community_by_symbol` map — semantic scoring still completes, just
without boundary leakage data.

### What NOT to change

- Do not modify `aether-graph-algo` (louvain_with_resolution_sync exists)
- Do not modify `aether-health` (planner and scoring are correct)
- Do not modify `aether-config` (use existing planner.community_resolution)
- Do not remove `list_louvain_communities()` (keep for compat)
- Do not change community_snapshot SQLite table schema
- Do not change semantic_signals.rs, archetypes.rs, or scoring.rs

### Tests

1. **cargo test -p aether-store**: Add or verify a test for the
   filtered Louvain method. Seed a graph with test and non-test
   symbols, verify test symbols are excluded from assignments and
   that resolution parameter affects community count.

2. **cargo test -p aether-analysis**: If existing drift tests mock
   or call list_louvain_communities, update for the new call. All
   existing tests must still pass.

3. **cargo test -p aetherd**: Update any test affected by the
   load_semantic_input signature change. All existing tests must pass.

## VALIDATION GATE

```bash
cargo fmt --check
cargo clippy -p aether-store -p aether-analysis -p aetherd -- -D warnings
cargo test -p aether-store
cargo test -p aether-analysis
cargo test -p aetherd
```

Then run on the real workspace:

```bash
pkill -f aetherd
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo build -p aetherd --release

$CARGO_TARGET_DIR/release/aetherd health-score \
  --workspace /home/rephu/projects/aether \
  --semantic --output table 2>&1
```

### Validation criteria

1. All tests pass, zero clippy warnings
2. Boundary Leaker count: was 11/16, must be at most 5
3. Overall score: was 43, must improve (target: 48+)
4. No regression in structural scores for any crate
5. If the score doesn't improve or gets worse, STOP and report the full
   output — do not commit

## COMMIT

```bash
git add -A
git commit -m "Fix Boundary Leaker false positives: global Louvain with test filtering + γ=0.5

- Add list_louvain_communities_filtered to SurrealGraphStore with
  configurable resolution and symbol exclusion set
- Add list_symbol_ids_with_paths helper for test symbol identification
- Update drift compute_boundary_records to use filtered Louvain with
  test symbols excluded and γ=0.5
- Update load_semantic_input to compute communities on the fly via
  filtered global Louvain instead of reading stale community_snapshot
- Test symbols identified via name/path heuristic (test_ prefix,
  tests/ directory)
- Boundary Leaker count drops from 11/16 to N (measured)
- Overall health score improves from 43 to M (measured)

Decision #89.1: global Louvain uses test filtering + configurable resolution"
```

Do NOT push. Robert will review the health-score output first.
