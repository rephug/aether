# Phase 8.18: Fix Boundary Leaker False Positives

## Purpose

Replace the naive global Louvain community lookup in health scoring with
file-scoped planner community detection. This eliminates the Boundary
Leaker false positives (11/16 crates flagged) that are dragging the
semantic score down by ~6 points.

## Prerequisites

- Phase 8.14 merged (component-bounded semantic rescue)
- Phase 8.15 merged (TYPE_REF + IMPLEMENTS edges)
- All five God File refactors merged

## Problem

The `boundary_leakage` metric in semantic health scoring counts the
fraction of files whose symbols span multiple communities. It is 20% of
`semantic_pressure`, which is ~30% of the overall score.

The metric reads community assignments from the `community_snapshot`
SQLite table. That table is populated by the drift pipeline
(`compute_boundary_records` in `drift.rs`), which runs **bare global
Louvain** (default γ=1.0) over every symbol in the workspace via
`list_louvain_communities()` on the SurrealDB graph store.

This global Louvain has none of the file-scoped planner improvements:
- No test filtering
- No type-anchor rescue
- No container rescue (empty-stem guard, etc.)
- No component-bounded semantic rescue
- No γ=0.5 tuning
- No stability checking

The result: symbols in the same file get assigned to different global
communities because the global Louvain groups by workspace-wide clusters,
not file-level responsibility. Most files touch 2+ global communities,
so `boundary_leakage_ratio` is sky-high, and 11/16 crates get the
Boundary Leaker archetype.

### How boundary_leakage flows into the score

```
load_semantic_input()              → reads community_snapshot from SQLite
  community_count per file         → count of distinct community IDs per file
semantic_signals.rs                → multi_community_files / indexed_file_count
  boundary_leakage                 → normalize_ratio(ratio, 0.50)
  semantic_pressure                → ... + boundary_leakage * 0.20
scoring.rs                         → semantic_bucket = semantic_pressure * 100
  combined_score                   → weighted avg with structural + git
```

## Fix

Replace the global `community_snapshot` lookup in `load_semantic_input()`
with file-scoped planner community detection. For each file, run
`detect_file_communities()` (the proven pipeline) with structural edges
and symbols. Use the resulting community count instead of the global
Louvain assignment.

### Why this works

- `detect_file_communities` is the same pipeline that produces the
  proven 11-community / 0.93-confidence results on aether-store
- It runs per-file: test filtering, type-anchor rescue, container rescue,
  Louvain at γ=0.5, stability checking — all included
- A file with symbols in 1 community has coherent responsibility → no flag
- A file with symbols in 3+ communities genuinely crosses boundaries → flag
- No embeddings needed: structural edges (calls, type_ref, implements)
  are sufficient for community counting. Semantic rescue won't fire
  without embeddings but that's fine — structural communities are
  meaningful on their own

### Why not fix the global Louvain instead

The global community snapshot is also consumed by drift boundary
violation analysis. Fixing the global pipeline would be a larger change
touching `drift.rs`, `coupling.rs`, and the SurrealDB graph store. That
can be done separately. This fix surgically targets the health score
without disrupting drift analysis.

## Scope

### Files to modify

```
crates/aetherd/src/health_score.rs — THE PRIMARY FILE
```

### What changes

1. **`load_semantic_input`** gains a `config: &AetherConfig` parameter
2. Build `FileCommunityConfig` from `config.planner` (same as suggest-splits)
3. After getting centrality data, open `SurrealGraphStore` and query all
   dependency edges (via `list_dependency_edges()`)
4. For each file: build `FileSymbol` entries (no embeddings), filter
   edges to that file's symbols, run `detect_file_communities`, count
   distinct community IDs in the result
5. Use that count as `community_count` in `SemanticFileInput`
6. **Remove** the `community_by_symbol` read from
   `store.list_latest_community_snapshot()` — no longer needed
7. Update the call site in `compute_current_report` to pass `config`

### What NOT to change

- `community_snapshot` SQLite table (still written by drift pipeline)
- `drift.rs` or `coupling.rs` (global pipeline is separate scope)
- `semantic_signals.rs` (the formula is correct, the input was wrong)
- `archetypes.rs` (Boundary Leaker threshold 0.6 is fine)
- `planner_communities.rs` or `planner_communities/` (no changes)
- `aether-health` crate (no changes)
- `aether-analysis` crate (no changes)
- `aether-config` crate (no changes)

## Implementation notes

### SurrealKV lock ordering

`load_semantic_input` creates a tokio runtime and calls
`centrality_by_file()` which opens SurrealDB, queries data, and drops
the connection. After that returns, the lock is released. A second
`SurrealGraphStore::open_readonly()` call to get edges is safe — no
concurrent access.

### Edge format conversion

`SurrealGraphStore::list_dependency_edges()` returns
`Vec<GraphDependencyEdgeRecord>` with fields `source_symbol_id`,
`target_symbol_id`, `edge_kind`. Convert to `GraphAlgorithmEdge` with
fields `source_id`, `target_id`, `edge_kind` for the planner.

### Building FileSymbol without embeddings

The existing `build_file_symbol` helper takes an `embedding_by_id` map.
Pass an empty `HashMap` — all symbols get `embedding: None`. Semantic
rescue won't fire, which is correct for this use case.

### Performance

Running `detect_file_communities` for every file in the workspace:
- ~200-400 files, ~10-50 symbols each
- Each planner call: 3 passes × in-memory graph operations
- Total: ~1-3 seconds for the entire workspace
- Acceptable for a health-score command that runs once

## Tests

1. **Existing tests must pass**: `cargo test -p aetherd` — all health
   score tests including the `semantic_input_community_count_*` tests

2. **Update `test_load_semantic_input_community_count`** (if it exists)
   to verify that community counts come from the planner, not the
   global snapshot. If it seeds `community_snapshot` and expects those
   counts, update it to verify planner-derived counts instead.

3. **New test: `boundary_leakage_uses_planner_communities`**
   Create a workspace with two files. File A has 6 symbols in 2 natural
   clusters (3 symbols calling each other, 3 separate symbols calling
   each other). File B has 4 symbols all connected. Verify:
   - File A: community_count >= 2
   - File B: community_count == 1
   - This is independent of any global `community_snapshot` data

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

Then run health score on the actual workspace:

```bash
pkill -f aetherd
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo build -p aetherd --release
$CARGO_TARGET_DIR/release/aetherd health-score \
  --workspace /home/rephu/projects/aether \
  --semantic --output table
```

### Expected improvement

- Boundary Leaker count: 11/16 → likely 2-4 (only files that genuinely
  have multi-community structure)
- Semantic pressure drops for most crates
- Overall score: 43/100 → estimated 48-55/100

## Decision to lock

**#89.1: boundary_leakage uses file-scoped planner communities, not global Louvain**

The global `community_snapshot` table is no longer used for health scoring.
File-scoped planner community detection produces accurate per-file
community counts that match the proven pipeline (8.12-8.14). The global
snapshot remains for drift boundary violation analysis (separate concern).
