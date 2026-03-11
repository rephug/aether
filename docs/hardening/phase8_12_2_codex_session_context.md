# Phase 8.12.2 — Continuation Session Context (Diagnostics + Bucketing Fix)

**Date:** 2026-03-10
**Branch:** `feature/phase8-anchor-split` (EXISTING — do not recreate)
**Worktree:** `/home/rephu/projects/aether-phase8-anchor-split` (EXISTING — do not recreate)
**Starting commit:** `01c2ab2` — Codex implementation (rarest-token bucketing)
**Goal:** Add per-step diagnostics to ablation AND replace rarest-token bucketing with
first-token bucketing. Both changes in one commit on top of `01c2ab2`.

This is a continuation of the existing branch. The previous iteration passed
fmt/clippy/tests but did NOT meet ablation acceptance criteria. The branch was
intentionally kept unmerged as the base for this narrow fix.

---

## Problem summary

aether-store/src/lib.rs (5869 LOC, ~214 non-test symbols) produces 3 communities with a
130-member largest blob. Target is 5-16 communities with largest < 100. A manual prototype
using first-token bucketing proved 16 communities / 92 largest / 0 loners is achievable.

The current implementation uses rarest-token bucketing, which produces 56 micro-buckets.
The small-bucket merge absorbs these into one mega-blob.

## What exists on the branch (commit 01c2ab2)

These functions were added by the previous Codex run. They EXIST on the branch but NOT
on main. The Codex prompt below will modify them:

- `split_large_anchor_groups()` — partitions large anchor groups by domain token
- `rebuild_union_find_from_groups()` — fresh DSU from final groups
- `apply_container_rescue_with_exclusions()` — skips split-anchor members
- `apply_container_rescue()` — wrapper for backward compat
- Both `run_detection` AND `run_ablation_pass` — patched to use split + exclusions
- Helper functions: `normalize_anchor_token`, `informative_tokens`,
  `informative_compound`, `token_overlap`
- Constants: `ANCHOR_SPLIT_THRESHOLD`, `ANCHOR_MIN_BUCKET`, `ANCHOR_STOPWORDS`
- 6 new tests, all passing

## What this session changes

### Part A: Per-step diagnostics in ablation

Add diagnostic prints to `run_ablation_pass` that show the pipeline state after each step.
This is the #1 priority because we currently have zero visibility into which sub-step
causes the collapse between rows 3 and 4 of the ablation table.

Print after each step:
1. After `build_anchor_groups` + `split_large_anchor_groups` + `rebuild_union_find_from_groups`
2. After `collapse_structural_edges`
3. After `apply_container_rescue_with_exclusions`
4. After `apply_semantic_rescue`
5. After `connected_components` (number of components, largest component size in reps)
6. After Louvain (community count before merge)

Format: `[diag] step_name: components=N largest=M reps_total=K`

### Part B: Replace rarest-token bucketing with first-token bucketing

Modify `split_large_anchor_groups` to use the first informative (non-stopword) token
from the method name as the bucket key, instead of the rarest token.

**Why first-token works:** `upsert_sir_meta` → `sir`, `list_project_notes` → `project`,
`run_migrations` → `migration`, `resolve_drift_result` → `drift`. This naturally produces
~10 domain-aligned buckets.

**Why rarest-token fails:** Compound tokens like `sir_history`, `sir_meta`, `sir_blob`
are each unique enough to be their own bucket. 56 micro-buckets → small-bucket merge
absorbs them into a mega-blob.

**Implementation:** The bucket key for a method name like `upsert_sir_meta` should be:
1. Split the method name by `_`
2. Filter out stopwords (the existing `ANCHOR_STOPWORDS` list)
3. Take the FIRST remaining token as the bucket key
4. If no informative token remains, use `"misc"` as the fallback bucket

Keep the existing `ANCHOR_MIN_BUCKET` threshold (currently 3). Keep the existing
small-bucket merge that absorbs tiny buckets into the nearest large bucket.

## File being modified

Only one file: `crates/aether-health/src/planner_communities.rs`

## Build / test / lint

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

# Before running ablation, check vector_backend:
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
# Must be "sqlite", NOT "lancedb"

# Remove stale SurrealDB lock if needed:
rm -f /home/rephu/projects/aether/.aether/graph/LOCK

# Test + lint
cargo fmt --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health

# Ablation (prints diagnostic output)
cargo test -p aether-health -- ablation_aether_store --ignored --nocapture
```

## Acceptance criteria

- Ablation diagnostics print per-step component count and largest component size
- Bucketing uses first informative token, not rarest token
- aether-store ablation: 5-16 communities, largest < 100, 0 loners
- All existing unit tests pass (52 original + 6 from 01c2ab2)
- Zero clippy warnings
- aether-mcp and aether-config ablations do not regress
