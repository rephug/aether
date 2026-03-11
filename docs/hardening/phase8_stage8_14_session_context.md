# Phase 8.14 — Semantic Rescue Stabilization — Session Context

**Date:** 2026-03-11
**Branch:** `feature/phase8-stage8-14-semantic-rescue` (to be created)
**Worktree:** `/home/rephu/projects/aether-phase8-semantic-rescue` (to be created)
**Starting commit:** HEAD of main (after 8.13 merge)

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

## What just merged

- **Phase 8.12** — Community detection pipeline
- **Phase 8.12.2** — First-token bucketing, empty-stem guard, per-step diagnostics
  - aether-store: 2/155 → 14/91/10 (communities/largest/loners), stability 0.97
- **Phase 8.13** — Symbol reconciliation + orphan cleanup for --full re-index

## The problem being solved

Semantic rescue in the file-scoped community detection pipeline acts as a
cross-component bridge instead of a local densifier. The 8.12.2 diagnostics proved:

```
Row 4 (+ rescue), baseline pass:
after_structural_edges:  components=54  largest_component=89
after_container_rescue:  components=45  largest_component=109  rescued=8
after_semantic_rescue:   components=9   largest_component=204  rescued=23  ← BRIDGE
connected_components:    count=1        sizes=[112]

Row 4 (+ rescue), perturbed pass (+0.05 threshold):
after_semantic_rescue:   components=33  largest_component=138  rescued=6
connected_components:    count=4        sizes=[73, 8, 5, 5]
```

A 0.05 threshold shift changes rescued symbols from 23 to 6 and connected
components from 1 to 4. The 14-community result at γ=0.5 happens to be stable
(0.97) because Louvain compensates, but the internal pipeline is volatile.

### Three identified bridging mechanisms

1. **Degree-1 grappling hook:** A symbol with degree 1 is already in Component A.
   If semantic rescue finds a match in Component B, it creates an A-B bridge.

2. **Y-bridge via max_k=3:** A degree-0 orphan casting 3 edges to 3 different
   components becomes a transit hub fusing all 3.

3. **Multi-symbol rep spray:** The loop iterates over symbols not reps. A degree-0
   rep with 3 symbols can cast 3 edges to 3 components because the degree check
   uses the rep but the loop advances by symbol.

### What was tried and reverted in 8.12.2

Constraining to `degree == 0` + `take(1)` destroyed stability (0.97 → 0.33) and
increased loners from 10 to 28. The three internal stability passes produced 9, 17,
and 26 communities. Too aggressive — a single edge per orphan makes rescue fragile.

## Locked design decision: Component-bounded rescue

**Approach:** Compute connected components BEFORE semantic rescue runs. Then
constrain semantic rescue to only add edges WITHIN the same connected component.

For orphans in singleton components (degree-0, no structural connection to anything):
- Find the best semantic match in ANY component
- Add a single edge absorbing the orphan INTO that component
- This is absorption, not bridging — the orphan joins one component, it cannot
  connect two existing components

**Why this works:**
- A symbol already inside Component A can only get semantic edges to other symbols
  in Component A. No cross-component bridges possible.
- A true orphan (singleton component) gets absorbed into exactly one component.
  It becomes a leaf node. Leaves cannot be bridges.
- max_k can stay at its current value for within-component rescue (local densification
  is fine). Only cross-component edges are blocked.

**Why the reverted approach failed but this won't:**
- `degree == 0 + take(1)` failed because it made ALL rescues fragile, not just
  cross-component ones. Within-component rescue (which is beneficial) was also
  crippled.
- Component-bounded rescue preserves within-component rescue at full strength
  while surgically blocking only cross-component bridges.

## Scope: Part A only

This stage covers ONLY semantic rescue refinement in the file-scoped planner.
Global community snapshot improvements (Boundary Leaker fix, orphaned subgraph
reduction) are deferred to a separate stage.

## Scope guard (must NOT be modified)

- Bucketing, anchor split, or naming logic (settled in 8.12/8.12.2)
- Health scoring formulas
- Edge extraction in aether-parse
- Store trait or implementations
- Container rescue logic (empty-stem guard is correct)
- Global community snapshot
- Coupling, drift, dashboard code
- Do NOT remove the `[diag]` prints from ablation

## Key files

```
crates/aether-health/src/planner_communities.rs  — semantic rescue + run_detection + run_ablation_pass
```

Only one file should need changes. The `apply_semantic_rescue` function and the
pipeline call sites in `run_detection` and `run_ablation_pass`.

## How to run ablation

```bash
# MUST check vector_backend first
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
# Must say "sqlite"

# Remove stale SurrealDB lock if needed
rm -f /home/rephu/projects/aether/.aether/graph/LOCK

# Run from the worktree
cd /home/rephu/projects/aether-phase8-semantic-rescue

# Run ablation
cargo test -p aether-health -- ablation_aether_store --ignored --nocapture 2>&1
```

## Acceptance criteria

After the change, the ablation must show:

- **Communities:** 10+ on aether-store (was 14 before, may shift slightly)
- **Largest:** < 100
- **Stability:** >= 0.90 (was 0.97 before — must not regress significantly)
- **Semantic rescue component reduction:** <= 30% (currently reduces 80%: 45 → 9)
- **All 3 internal stability passes** should produce similar community counts
  (not 9/17/26 like the reverted attempt)
- All existing unit tests pass
- Zero clippy warnings

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
