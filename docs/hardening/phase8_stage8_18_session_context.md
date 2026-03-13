# Phase 8.18 — Boundary Leaker Fix — Session Context

**Date:** 2026-03-13
**Branch:** `feature/phase8-stage8-18-boundary-leaker-fix` (to be created)
**Worktree:** `/home/rephu/aether-phase8-boundary-leaker` (to be created)
**Starting commit:** HEAD of main (f5bd037 — batch embedding meta lookup fix)

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

| Commit | What |
|--------|------|
| f5bd037 | Batch embedding meta lookup (ARCH-2 N+1 fix) |
| b9834e4 | Batch progress logging for SIR generation |
| f6150b8 | Triage/deep pass concurrency fix (14x speedup) |
| 4e0917c | Refactor aether-mcp (last of 5 God File refactors) |
| 9f07651 | Phase 8.17 — Gemini native embedding provider |
| 02b5d61 | Phase 8.16 — --embeddings-only + OpenAI-compat provider |
| 50c174e | Phase 8.15 — TYPE_REF + IMPLEMENTS edge extraction |
| 1f94993 | Phase 8.14 — Component-bounded semantic rescue |

## The problem being solved

The `boundary_leakage` metric in health scoring is producing false
positives. 11/16 crates are flagged as Boundary Leakers.

Root cause: `community_count` per file is derived from the
`community_snapshot` SQLite table, which stores global Louvain community
assignments computed by the drift pipeline. Global Louvain has none of
the file-scoped planner improvements — no test filtering, no rescue
passes, no γ=0.5 tuning. Symbols in the same file get assigned to
different global communities, inflating boundary_leakage.

### How boundary_leakage feeds the score

```
load_semantic_input (health_score.rs)
  → reads community_snapshot from SQLite
  → community_count = distinct community IDs per file's symbols
semantic_signals.rs
  → boundary_leakage = multi_community_files / indexed_file_count
  → normalized against boundary_leakage_high (0.50)
  → weighted at 20% of semantic_pressure
scoring.rs
  → semantic_pressure → semantic bucket (30% of overall score)
```

With 11/16 crates flagged, boundary_leakage is near-max for most crates,
dragging the workspace score from an estimated ~50 down to 43.

### Key data points

- Overall score: 43/100 (Watch)
- aether-store: 77/100 after refactor (God File resolved)
- Boundary Leaker archetype: 11/16 crates
- Global Louvain: runs on full workspace graph, γ=1.0, no planner features
- File-scoped planner: 11 communities / 131 largest / 3 loners / 0.93
  confidence / 0.82 stability on aether-store

## The fix

Replace the global `community_snapshot` lookup in `load_semantic_input()`
with file-scoped `detect_file_communities()` calls.

For each file:
1. Get symbols from SQLite
2. Filter structural edges to that file's symbol set
3. Build FileSymbol entries (without embeddings — pass empty map)
4. Call `detect_file_communities()` with the file's edges and symbols
5. Count distinct community IDs in the result
6. Use that as `community_count` in SemanticFileInput

Remove the `community_by_symbol` read from
`store.list_latest_community_snapshot()`.

## Scope guard (must NOT be modified)

- `community_snapshot` SQLite table schema
- `drift.rs` or `coupling.rs` (global community pipeline)
- `semantic_signals.rs` (formula is correct, input was wrong)
- `archetypes.rs` (Boundary Leaker threshold 0.6 is fine)
- `planner_communities.rs` or any file in `planner_communities/`
- `aether-health` crate
- `aether-analysis` crate
- `aether-config` crate
- `aether-store` crate

## Key file

```
crates/aetherd/src/health_score.rs — THE ONLY FILE TO MODIFY
```

## How to run health score

```bash
pkill -f aetherd
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo build -p aetherd --release

$CARGO_TARGET_DIR/release/aetherd health-score \
  --workspace /home/rephu/projects/aether \
  --semantic --output table
```

## Acceptance criteria

After the change:
- `cargo test -p aetherd` passes (all existing tests)
- Health score on real workspace: Boundary Leaker count drops from 11
  to at most 4-5 (only files with genuine multi-community structure)
- Overall score improves (estimated 48-55, was 43)
- No regression in structural or git scores

## Decision register note

Highest confirmed decision: #89 (Phase 8.15/8.16 addendum, Gemini
native provider). This fix takes #89.1.

Phase 9 decisions start at #90:
- #90: Tauri 2.x as desktop framework
- #91: Frontend stays HTMX + D3
- #92: Single binary embeds daemon
- #93: System tray as primary status surface
- #94: Platform installers via cargo tauri build
- #95: Auto-update via Tauri updater plugin
- #96: Alert tray state (reserved for Phase 11 Sentinel)

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
