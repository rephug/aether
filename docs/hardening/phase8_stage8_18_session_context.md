# Phase 8.18 — Boundary Leaker Fix (Global Pipeline) — Session Context

**Date:** 2026-03-13
**Branch:** `feature/phase8-stage8-18-boundary-leaker-fix` (to be created)
**Worktree:** `/home/rephu/aether-phase8-boundary-leaker` (to be created)
**Starting commit:** HEAD of main (f5bd037 — batch embedding meta lookup fix)

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

## What was tried and failed

A prior attempt replaced the global community_snapshot lookup in
load_semantic_input with per-file detect_file_communities calls. Result:
36/100 overall score (was 43), 13/16 Boundary Leakers (was 11).

Root cause: the file-scoped planner only sees intra-file edges. Most
files have 10 symbols with 2-3 intra-file edges, so the planner
fragments them into multiple tiny "communities." Cross-file edges
(which unify symbols architecturally) are invisible to the planner.

## The correct fix: improve the global Louvain pipeline

The global Louvain in `list_louvain_communities()` runs at γ=1.0 with
no test filtering. The file-scoped planner runs at γ=0.5 with test
symbols excluded. Porting these two improvements to the global pipeline
produces architecturally-meaningful communities that correctly reflect
whether a file crosses module boundaries.

### Key existing code

- `louvain_with_resolution_sync(edges, resolution)` — already exists
  in `aether-graph-algo`, is `pub`, but NOT imported in `graph_surreal.rs`
- `list_louvain_communities()` in `graph_surreal.rs` — calls
  `louvain_sync` (which wraps `louvain_with_resolution_sync(edges, 1.0)`)
- `list_dependency_edges_raw()` — returns all structural edges
- `to_algo_edges()` — converts DependencyEdgeRow to GraphAlgorithmEdge
- SurrealDB symbol table has `symbol_id`, `qualified_name`, `file_path`
- `compute_boundary_records` in `drift.rs` calls
  `cozo.list_louvain_communities()` and writes `community_snapshot`
- `load_semantic_input` in `health_score.rs` reads `community_snapshot`

### Files to modify

```
crates/aether-store/src/graph_surreal.rs     — new method + import
crates/aether-store/src/graph_cozo_compat.rs — compat wrapper
crates/aether-analysis/src/drift.rs          — update call site
crates/aetherd/src/health_score.rs           — compute communities on the fly
```

## Scope guard (must NOT be modified)

- `aether-graph-algo` crate
- `aether-health` crate
- `aether-config` crate
- `community_snapshot` SQLite table schema
- `semantic_signals.rs`, `archetypes.rs`, `scoring.rs`
- The existing `list_louvain_communities()` method (keep for compat)
- The file-scoped planner in `planner_communities/`

## Acceptance criteria

After the change:
- `cargo test -p aether-store` passes
- `cargo test -p aether-analysis` passes
- `cargo test -p aetherd` passes
- Health score on real workspace: Boundary Leaker count drops from 11
  to at most 5
- Overall score improves above 43
- No regression in structural or git scores
- If score doesn't improve, STOP and report output

## Decision register

Highest confirmed: #89. This fix takes #89.1.
Phase 9 decisions start at #90 (Tauri 2.x) through #96.

## End-of-stage git sequence

```bash
cd /home/rephu/aether-phase8-boundary-leaker
git push origin feature/phase8-stage8-18-boundary-leaker-fix

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-boundary-leaker
git branch -D feature/phase8-stage8-18-boundary-leaker-fix
git worktree prune
```
