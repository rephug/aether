# Phase 8 — Stage 8.14: Semantic Rescue Stabilization + Global Community Quality

## Purpose

Stabilize semantic rescue so it acts as a local densifier within structural
components rather than a cross-component bridge. Then apply the proven
file-scoped pipeline improvements to the global community snapshot used by
drift detection and boundary violation analysis.

## Prerequisites

- Phase 8.12 merged (file-scoped community detection pipeline)
- Phase 8.12.2 merged (first-token bucketing, empty-stem guard, per-step diagnostics)
- Phase 8.13 merged (symbol reconciliation — independent but should ship first)

## Evidence from 8.12.2 diagnostics

The `[diag]` prints added in 8.12.2 proved the exact pipeline behavior on
aether-store (5869 LOC, ~214 non-test symbols). The merged state (14/91/10)
is stable, but the diagnostics exposed a known remaining issue.

### Semantic rescue cross-component bridging (proven)

Row 4 of the ablation (type-anchor + rescue) at the current threshold:

```
after_structural_edges:  components=54  largest_component=89
after_container_rescue:  components=45  largest_component=109  rescued=8
after_semantic_rescue:   components=9   largest_component=204  rescued=23  ← BRIDGE
connected_components:    count=1        sizes=[112]
after_louvain:           communities=14-23 (varies by pass)
```

The perturbed pass (threshold +0.05) shows different behavior:

```
after_semantic_rescue:   components=33  largest_component=138  rescued=6
connected_components:    count=4        sizes=[73, 8, 5, 5]
after_louvain:           communities=9-18 (varies)
```

A 0.05 threshold shift changes rescued symbols from 23 to 6 and connected
components from 1 to 4. The 14-community result at γ=0.5 is stable (0.97),
but the pipeline is sensitive to threshold tuning.

### Three specific bridging mechanisms (identified but NOT yet fixed)

1. **Degree-1 grappling hook:** A symbol with degree 1 is already in
   Component A. If semantic rescue finds a match in Component B, it draws
   an edge between A and B, merging them. The `degree <= 1` check was
   designed to help weakly-connected symbols, but it enables bridging.

2. **Y-bridge via max_k=3:** A degree-0 orphan casting 3 semantic edges to
   3 different components becomes a transit hub that fuses all 3 into one
   mega-blob.

3. **Multi-symbol rep spray:** The rescue loop iterates over symbols, not
   reps. A degree-0 rep containing 3 symbols (from container rescue) can
   cast 3 edges to 3 different components because the degree check uses
   the rep but the loop advances by symbol.

### What was tried and reverted (proven too aggressive)

Constraining semantic rescue to `degree == 0` + `take(1)` destroyed
stability (0.97 → 0.33) and increased loners from 10 to 28. The problem:
with only 1 edge per orphan, whether that orphan gets rescued depends
entirely on its single best match clearing the threshold. A +0.05 shift
flips most rescues, producing wildly different community counts across
stability passes (9, 17, 26 from the same input).

### 10 loners are passive record types (explained)

The remaining 10 loners (`SirMetaRecord`, `DriftResultRecord`,
`ProjectNoteRecord`, etc.) are top-level data definitions with zero
outgoing `CALLS` edges. They are correctly left as loners by the pipeline.
They will resolve naturally when 8.15 adds `TYPE_REF` edges that connect
them to the methods that use them as parameters/return types.

## What this stage does

### Part A: Semantic rescue refinement (file-scoped planner)

Design a component-aware semantic rescue that prevents cross-component
bridging while maintaining stability. The key constraint: a rescued symbol
should be absorbed INTO an existing component, never BRIDGE two components.

**Approach options to investigate (ranked by evidence strength):**

1. **Component-bounded rescue:** Compute connected components BEFORE semantic
   rescue. Only allow semantic edges within the same pre-rescue component.
   Orphans in singleton components get rescued into the nearest component
   by best semantic match (one edge, one direction — absorption not bridging).
   This is the most principled approach but needs ablation validation.

2. **Degree-0 only + take(2):** Tighten from `degree <= 1` to `degree == 0`
   but keep `take(2)` instead of `take(1)`. Two edges provide redundancy
   (if one match is borderline, the other stabilizes it) without enabling
   Y-bridging to 3+ components. Simpler than component-bounded but still
   allows cross-component bridges if both targets are in different components.

3. **Dynamic re-check after each rescue:** After adding an edge for one
   symbol, re-check `graph.degree(source_rep)` before processing the next
   symbol in the same rep. This prevents multi-symbol rep spray but doesn't
   address the degree-1 grappling hook.

**Validation method:** Use the existing `[diag]` prints in `run_ablation_pass`.
Each approach must be tested against the ablation and must achieve:
- 10+ communities on aether-store
- Largest < 100
- Stability >= 0.90
- Semantic rescue should NOT reduce components by more than ~30% (currently
  it reduces from 45 to 9 — a 80% reduction, which is excessive)

### Part B: Global community snapshot improvements

Apply the file-scoped pipeline lessons to the global `mine-coupling →
Louvain → community_snapshot` pipeline. This fixes:

- **Boundary Leaker false positives** (11/16 crates flagged, most incorrect)
- **Orphaned subgraph counts** (63 orphaned subgraphs in self-analysis)

**Changes:**
- Apply empty-stem guard to global container rescue (if applicable)
- Apply test filtering before global Louvain
- Consider component-bounded semantic rescue for global scope
- Keep global γ = 1.0 unless ablation shows a better value
- Backward-compat: drift-report and boundary violation analysis consume
  the global snapshot — any changes must not break existing behavior

## Scope guard

- Do NOT change the file-scoped planner pipeline's bucketing, anchor split,
  or naming logic (those are settled in 8.12/8.12.2)
- Do NOT change health scoring formulas
- Do NOT change edge extraction in aether-parse (that's 8.15)
- Do NOT change Store trait or implementations
- Do NOT remove the `[diag]` prints from ablation — they're essential for
  validating this stage's changes

## Key files

```
crates/aether-health/src/planner_communities.rs  — semantic rescue refinement
crates/aether-analysis/src/coupling.rs           — global community pipeline
crates/aether-store/src/lib.rs                   — community_snapshot table (read-only ref)
crates/aether-health/src/archetypes.rs           — BoundaryLeaker assignment
```

## Tests

### Part A (semantic rescue)
- Existing ablation tests (ablation_aether_store, ablation_aether_mcp,
  ablation_aether_config) must not regress
- New test: `semantic_rescue_does_not_bridge_components` — two disconnected
  components with a degree-0 orphan between them; after rescue, the orphan
  joins one component but does not merge them
- New test: `semantic_rescue_stability_under_threshold_perturbation` —
  stability score >= 0.90 with threshold ±0.05

### Part B (global community)
- `boundary_leaker_count_reduced_after_global_fix` — re-run self-analysis,
  expect fewer than 5 Boundary Leakers (was 11)
- `orphaned_subgraph_count_reduced` — expect fewer than 20 (was 63)
- `drift_report_backward_compatible` — existing drift results unchanged
  after global snapshot update

## Decisions to lock

- **#72**: Semantic rescue must not reduce connected components by more
  than 30% (prevents catastrophic bridging while allowing local densification)
- **#73**: Component-bounded rescue vs degree-0+take(2) — decide after
  ablation comparison
- **#74**: Global community snapshot update strategy (full rebuild vs
  incremental)

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aether-health -p aether-analysis -p aetherd -- -D warnings
cargo test -p aether-health
cargo test -p aether-analysis
cargo test -p aetherd

# Ablation validation
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
cargo test -p aether-health -- ablation_aether_store --ignored --nocapture
```

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-semantic-rescue
git push origin feature/phase8-stage8-14-semantic-rescue

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-semantic-rescue
git branch -D feature/phase8-stage8-14-semantic-rescue
git worktree prune
```
