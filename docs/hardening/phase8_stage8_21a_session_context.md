# Phase 8.21a — Two-Signal Trait Clustering — Session Context

**Date:** 2026-03-14
**Branch:** `fix/trait-cluster-two-signal` (to be created)
**Worktree:** `/home/rephu/aether-fix-trait-cluster` (to be created)
**Starting commit:** HEAD of main after 8.19a merge

## CRITICAL: Read actual source, not this document

```bash
/home/rephu/projects/aether
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

## What just merged

| Commit | What |
|--------|------|
| (8.19a) | Bare method name matching in usage_matrix + type-level deps |
| (8.21) | Trait split planner — consumer-bitvector clustering |
| (8.20) | method_dependencies in SIR schema |
| (8.19) | usage_matrix tool, type-level dep aggregation, search fallback |

## The problem being solved

`aether_suggest_trait_split` on Store (59 methods, 65 consumers) produces
"low" confidence with mostly singleton clusters. The consumer-only
clustering creates a singleton explosion because each method has a nearly
unique consumer fingerprint.

Three manual AI-produced plans independently converged on 11 sub-traits
using domain reasoning (shared dependency types), not consumer patterns.
Deepthink analysis confirmed: dependency types should be the primary
clustering signal (0.75 weight), consumer patterns secondary (0.25).

## The current algorithm (what to replace)

In `crates/aether-health/src/planner.rs`, `suggest_trait_split`:

1. Groups methods by **identical** consumer sets → BTreeMap
2. Calls `merge_similar_trait_clusters` which merges **singletons only**
   into existing clusters via consumer Jaccard >= 0.80
3. Uses `method_dependencies` only for **naming**, not clustering

The replacement:

1. Build filtered dependency sets per method (strip ubiquitous types)
2. If <50% of methods have dep data, fall back to current consumer-only
3. Otherwise, run agglomerative hierarchical clustering (average linkage)
   with fused score: `0.75 * dep_jaccard + 0.25 * consumer_jaccard`
4. Stop merging when best score < 0.30
5. method_dependencies now drives **both** clustering AND naming

## Key file

Only one file changes: `crates/aether-health/src/planner.rs`

Read the full `suggest_trait_split` function (starts ~line 202) and
the helper functions it calls:

- `merge_similar_trait_clusters` (~line 580) — REPLACE
- `jaccard_similarity` (~line 698) — REUSE
- `dominant_dependencies_for_cluster` (~line 728) — REUSE
- `method_dependency_values` (~line 764) — REUSE
- `display_dependency_name` (~line 789) — REUSE
- `cluster_consumer_union` (~line 678) — REUSE
- `cluster_isolation` (~line 690) — REUSE

## Existing tests to preserve

The planner.rs test module has tests for suggest_trait_split:
- Two exact consumer clusters
- Overlapping consumers {1} {2,3} {4}
- Cross-cutting detection
- Zero-caller methods in uncalled_methods
- Dependency-driven naming

These tests pass `method_dependencies: None`, so they exercise the
fallback path. They MUST still pass after the change — the fallback
path preserves current behavior.

## Scope guard

- ONLY modify `crates/aether-health/src/planner.rs`
- Do NOT change the `TraitSplitSuggestion` return type
- Do NOT change the MCP tool or CLI integration
- Do NOT change naming logic (it already works well)
- The agglomerative algorithm replaces the clustering step only
- Existing consumer-only tests must still pass via the fallback path

## After this merges

```bash
git push -u origin fix/trait-cluster-two-signal
# Create PR via GitHub web UI
# After merge:
cd ~/projects/aether
git pull --ff-only
git worktree remove /home/rephu/aether-fix-trait-cluster
git branch -d fix/trait-cluster-two-signal
cargo build -p aether-mcp --release
```

Then validate against Store (after SIR regeneration populates method_dependencies):
```bash
# In Codex:
# "Call aether_suggest_trait_split for Store in crates/aether-store/src/lib.rs"
# Expected: 8-12 clusters, Medium or High confidence
```
