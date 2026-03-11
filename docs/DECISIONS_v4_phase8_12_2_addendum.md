# DECISIONS_v4 — Phase 8.12.2 Addendum

**Date:** 2026-03-10
**Context:** Phase 8.12.2 (Large Anchor Intra-Partitioning) — community detection
collapse diagnosis and fix.

**Numbering note:** Continues from the Phase 8.12 decisions (#59-#71).

---

## New decisions (Phase 8.12.2 — Implemented)

### 72. First-informative-token bucketing replaces rarest-token

**Date:** 2026-03-10
**Status:** ✅ Implemented (commit f60c603)

**Context:** Codex's rarest-token bucketing produced 56 micro-buckets for
aether-store's 141 SqliteStore methods. Compound tokens like `sir_history`
and `sir_blob` were globally rare enough to be their own bucket. The
small-bucket merge absorbed 53 tiny buckets into one 67-member mega-blob
via a size tiebreaker when token overlap was 0.

A manual Python prototype using simple first-token bucketing achieved the
target: 16 communities, 92 largest, 0 loners.

**Decision:** Use the first non-stopword token from the method name as the
bucket key. `upsert_sir_meta` → `sir`, `list_project_notes` → `project`,
`resolve_drift_result` → `drift`. This naturally produces 8-12 medium-sized
domain-aligned buckets.

**Implementation:** `split_large_anchor_groups()` in
`crates/aether-health/src/planner_communities.rs`.

### 73. Empty stems excluded from container rescue

**Date:** 2026-03-10
**Status:** ✅ Implemented (commit 88f0afa)

**Context:** Per-step diagnostics proved that container rescue was the
catastrophic collapse point. After structural edges, aether-store had 54
components / 89 largest. Container rescue dropped it to 3 components / 203
largest by rescuing 48 symbols.

Root cause: `qualified_name_stem()` returns empty string `""` for top-level
types like `SirMetaRecord`, `DriftResultRecord`, `ProjectNoteRecord` (no
`::` in their qualified name). Container rescue grouped all ~20+ unrelated
record types via the shared empty stem, bridging them together. These
records had structural edges into SqliteStore sub-groups, collapsing
everything.

**Decision:** Skip entries with empty stems in container rescue. Empty stems
are not meaningful container signals. Those types are handled by Louvain
and merge_small_communities based on structural/semantic evidence.

**Implementation:** Guard clause in `apply_container_rescue_with_exclusions()`
in `crates/aether-health/src/planner_communities.rs`.

### 74. Semantic rescue cross-component bridging: known issue, deferred to 8.14

**Date:** 2026-03-10
**Status:** ⏳ Deferred

**Context:** Diagnostics showed semantic rescue reducing aether-store from
45 components / 109 largest to 9 components / 204 largest by rescuing 23
symbols. A +0.05 threshold shift changed rescued count from 23 to 6,
producing wildly different community counts. The current 14/91/10 result
is stable (0.97) but is being held together by specific threshold tuning.

An attempt to constrain semantic rescue to `degree == 0` + `take(1)` was
implemented and reverted. It destroyed stability (0.97 → 0.33) and
increased loners from 10 to 28. The approach was too aggressive.

Three specific bridging mechanisms were identified:
1. Degree-1 grappling hook (already-connected symbol bridging two components)
2. Y-bridge via max_k=3 (orphan connecting 3 components simultaneously)
3. Multi-symbol rep spray (loop over symbols not reps)

**Decision:** Accept 14/91/10/0.97 as the merge state for 8.12.2. Defer
semantic rescue refinement to Stage 8.14, which will explore
component-bounded rescue as the principled fix. Do not attempt further
semantic rescue changes without a full ablation cycle.

### 75. Per-step diagnostics retained in ablation harness

**Date:** 2026-03-10
**Status:** ✅ Implemented (commit f60c603)

**Context:** The `[diag]` prints added to `run_ablation_pass` were essential
for diagnosing the container rescue collapse and semantic rescue bridging.
They print to stderr via `eprintln!` and only run in the test-only ablation
path — zero production overhead.

**Decision:** Keep the `[diag]` prints in the codebase. They are needed for
8.14 (semantic rescue refinement) and any future pipeline tuning.

**Implementation:** `run_ablation_pass()` in `mod tests` of
`crates/aether-health/src/planner_communities.rs`.

### 76. 10 loners are passive record types — resolved by TYPE_REF edges (8.15)

**Date:** 2026-03-10
**Status:** ⏳ Tracked

**Context:** The 10 loner symbols in aether-store are top-level data
definitions (`SirMetaRecord`, `DriftResultRecord`, `ProjectNoteRecord`,
etc.) with zero outgoing CALLS edges. They lack structural connections
to any community because `aether-parse` only extracts CALLS and DEPENDS_ON
edges.

**Decision:** These loners are handled correctly by Decision #65 (loners
excluded from suggestions, not fake-placed). They will resolve naturally
when Stage 8.15 adds TYPE_REF edge extraction, which connects passive
types to the methods that use them as parameters/return types. Do not
add heuristic workarounds in the planner.

---

## Summary of 8.12.2 ablation progression

| State | Communities | Largest | Loners | Stability |
|-------|-------------|---------|--------|-----------|
| 8.12 baseline | 2 | 155 | 0 | 1.00 |
| + rarest-token bucketing (Codex) | 3 | 130 | 0 | 0.99 |
| + first-token bucketing | 3 | 130 | 0 | 0.99 |
| + empty-stem guard | 14 | 91 | 10 | 0.97 |
| + semantic rescue constraints (reverted) | 15 | 93 | 28 | 0.33 |
| **Final merge state** | **14** | **91** | **10** | **0.97** |
| Prototype target | 16 | 92 | 0 | — |
