# Phase 8.18: Fix Boundary Leaker False Positives — Global Pipeline

## Purpose

Port the proven planner improvements (test filtering + γ tuning) to the
global Louvain community pipeline. This fixes both the health score
Boundary Leaker false positives (11/16 crates flagged) and improves the
drift boundary violation analysis.

## Prerequisites

- Phase 8.14 merged (component-bounded semantic rescue)
- Phase 8.15 merged (TYPE_REF + IMPLEMENTS edges)
- All five God File refactors merged

## Problem

The `boundary_leakage` metric in semantic health scoring counts the
fraction of files whose symbols span multiple global Louvain communities.
It is 20% of `semantic_pressure`, which is ~30% of the overall score.

The global Louvain (`list_louvain_communities()` in `graph_surreal.rs`)
runs with default γ=1.0 on unfiltered edges. This differs from the
file-scoped planner which uses γ=0.5 and filters test symbols. The
result:

1. **γ=1.0 over-splits:** Higher resolution produces many small
   communities instead of fewer architecturally-meaningful modules.
   Symbols in the same logical module get split across communities.

2. **Test symbol pollution:** Test functions and their edges distort
   community structure. The file-scoped planner filters these out;
   the global pipeline does not.

Combined effect: symbols in the same file get assigned to different
communities, inflating `boundary_leakage_ratio`. 11/16 crates are
flagged as Boundary Leakers.

### Why the file-scoped planner approach failed

A prior attempt replaced the global snapshot with per-file
`detect_file_communities()` calls. This made things worse (36/100,
13/16 Boundary Leakers) because the file-scoped planner only sees
intra-file edges. Most of a file's connectivity is cross-file (calls,
type_ref to symbols in other files). With only 2-3 intra-file edges
visible, the planner fragments every small file into multiple
"communities" — worse than the global Louvain.

The correct scope for "does this file cross architectural boundaries?"
is global: you need the full workspace graph to see how a file's
symbols relate to the rest of the codebase.

## Fix

### 1. Add filtered Louvain method to SurrealGraphStore

Add `list_louvain_communities_filtered` to `graph_surreal.rs`:

```rust
pub async fn list_louvain_communities_filtered(
    &self,
    resolution: f64,
    exclude_symbol_ids: &HashSet<String>,
) -> Result<Vec<(String, i64)>, StoreError>
```

Implementation:
- Get raw edges via `list_dependency_edges_raw()`
- Filter out edges where source OR target is in `exclude_symbol_ids`
- Run `louvain_with_resolution_sync(filtered_edges, resolution)`
- Return sorted assignments

This keeps the graph store dumb — callers decide what to exclude
and what resolution to use.

Import `louvain_with_resolution_sync` from `aether_graph_algo` (already
`pub`, just not imported in this file).

### 2. Add compat wrapper to CozoGraphStore

Add corresponding sync method to `graph_cozo_compat.rs`:

```rust
pub fn list_louvain_communities_filtered(
    &self,
    resolution: f64,
    exclude_symbol_ids: &HashSet<String>,
) -> Result<Vec<(String, i64)>, StoreError>
```

### 3. Update drift pipeline

In `drift.rs` `compute_boundary_records`:
- Gather test symbol IDs from SqliteStore using name/path heuristics
- Call `cozo.list_louvain_communities_filtered(resolution, &test_ids)`
  instead of `cozo.list_louvain_communities()`
- Resolution comes from config (use the same `planner.community_resolution`,
  default 0.5)

This means `compute_boundary_records` needs access to resolution config.
Thread it through the method signature or pass a default.

### 4. Update health scoring

In `health_score.rs` `load_semantic_input`:
- Add `config: &AetherConfig` parameter (same as the prior attempt)
- Open SurrealGraphStore, gather test symbol IDs, call the filtered
  Louvain method directly
- Use the resulting assignments for `community_count` per file
- Remove the `community_by_symbol` read from
  `store.list_latest_community_snapshot()`
- This makes health-score self-contained — no dependency on a prior
  drift analysis run

### Test symbol identification

At both call sites, identify test symbols using the name/path heuristic
already proven in `symbol_is_test()`:

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

This skips the `list_test_intents_for_symbol` SQLite lookup (too slow
for 3700+ symbols). The name/path heuristic catches the bulk of test
symbols. If SQLite test intent data is cheap to batch-query, it can be
added later.

For the SurrealDB path: query all symbol records to get their IDs,
qualified_names, and file_paths. Apply the heuristic. Collect excluded
IDs into a HashSet.

## Files to modify

```
crates/aether-store/src/graph_surreal.rs     — new filtered Louvain method
crates/aether-store/src/graph_cozo_compat.rs — compat wrapper
crates/aether-analysis/src/drift.rs          — update compute_boundary_records
crates/aetherd/src/health_score.rs           — update load_semantic_input
```

## What NOT to change

- `aether-graph-algo` crate (louvain_with_resolution_sync already exists)
- `aether-health` crate (planner and scoring formulas are correct)
- `aether-config` crate (use existing planner.community_resolution)
- `community_snapshot` SQLite table schema
- `semantic_signals.rs` or `archetypes.rs`
- The existing `list_louvain_communities()` method (keep it for
  backward compatibility; the new filtered method is additive)

## Tests

1. **New test in graph_surreal.rs or graph_cozo_compat.rs:**
   `louvain_filtered_excludes_test_symbols` — seed a graph with
   test and non-test symbols, verify that test symbols are excluded
   from the community assignments.

2. **New test in graph_surreal.rs or graph_cozo_compat.rs:**
   `louvain_filtered_uses_resolution` — seed a graph, run with
   γ=0.5 and γ=2.0, verify different community counts.

3. **Update test in drift.rs (if needed):**
   If `compute_boundary_records` tests mock the Louvain call,
   update them for the new method signature.

4. **Existing test in health_score.rs:**
   Update for the new `load_semantic_input` signature (add config).

5. **All existing tests must pass:**
   `cargo test -p aether-store`
   `cargo test -p aether-analysis`
   `cargo test -p aetherd`

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

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
  --semantic --output table
```

### Expected improvement

- Boundary Leaker count: 11/16 → at most 4-5
- Overall score: 43/100 → estimated 48-55
- No regression in structural or git scores

## Decision

**#89.1: Global Louvain uses test filtering + configurable resolution**

The global `list_louvain_communities_filtered` method accepts a resolution
parameter and a set of symbol IDs to exclude. Call sites use γ=0.5
(matching the file-scoped planner) and exclude test symbols via name/path
heuristics. Health scoring computes communities on the fly using this
method instead of reading the `community_snapshot` table.
