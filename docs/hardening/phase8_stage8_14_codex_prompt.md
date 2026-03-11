# Codex Prompt — Phase 8.14: Component-Bounded Semantic Rescue

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are implementing a focused change to ONE function in ONE file.

Read these files before writing any code:
- `docs/roadmap/phase_8_stage_8_14_semantic_rescue.md` (the full spec)
- `docs/hardening/phase8_stage8_14_session_context.md` (session context — has exact diagnostic data)
- `crates/aether-health/src/planner_communities.rs` (THE ONLY FILE TO MODIFY)

Pay close attention to:
- `apply_semantic_rescue` — the function being changed
- `run_detection` — production pipeline, calls apply_semantic_rescue
- `run_ablation_pass` — test pipeline, calls apply_semantic_rescue
- The `[diag]` prints in run_ablation_pass — these validate the change

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add ../aether-phase8-semantic-rescue -b feature/phase8-stage8-14-semantic-rescue
cd /home/rephu/projects/aether-phase8-semantic-rescue
```

## SOURCE INSPECTION

Before writing code, inspect `apply_semantic_rescue` and verify these assumptions
in your reasoning. If any assumption is false, STOP and report:

1. Source selection uses `degree <= 1` (or similar low-degree check)
2. `semantic_rescue_max_k` controls how many edges per rescued symbol
3. The function receives the enriched graph AFTER container rescue
4. Connected components are computed AFTER semantic rescue in the current pipeline

## IMPLEMENTATION

### The change: Component-bounded semantic rescue

Modify `apply_semantic_rescue` (or add a new variant) so that semantic edges
can only be added WITHIN the same pre-rescue connected component.

**Algorithm:**

1. **Before the rescue loop:** Compute connected components on the current graph
   state (after container rescue, before semantic rescue). Build a map:
   `rep → component_id` for every active rep.

2. **Classify each source symbol:**
   - If the source rep's component has 2+ reps → it's an "in-component" symbol.
     Only allow semantic targets within the SAME component.
   - If the source rep's component has exactly 1 rep (singleton) → it's a true
     orphan. Allow it to find the best semantic match in ANY component, but limit
     to `take(1)` — one edge, pure absorption, the orphan becomes a leaf.

3. **Within-component rescue** (non-singleton sources):
   - Keep the existing degree check (`degree <= 1` or whatever the source has)
   - Keep the existing `semantic_rescue_max_k` limit
   - Filter candidates to only those whose rep is in the same component
   - This is local densification — it cannot bridge components

4. **Orphan absorption** (singleton sources):
   - Find the single best semantic match, **preferring targets in non-singleton
     components**. Only fall back to singleton targets if no non-singleton
     candidate clears the threshold.
   - Add exactly 1 edge (`take(1)`)
   - The orphan joins that component as a leaf node
   - Leaves cannot be bridges (graph theory guarantee)

**Hard rules (apply to both paths):**
- Never add a semantic edge where `source_rep == target_rep`
- Never add a duplicate edge (check if edge already exists between the two reps)
- **Dynamic re-check:** After one semantic edge is added from a `source_rep`,
  re-check its degree before processing additional symbols in the same rep.
  This prevents multi-symbol reps from spraying edges (one of the three
  identified bridging mechanisms from 8.12.2).

**Important:** Both `run_detection` and `run_ablation_pass` must use the same
rescue logic. Do not create divergent code paths.

### What NOT to change

- Do not change the degree threshold (`<= 1`) for within-component rescue
- Do not change `semantic_rescue_max_k` for within-component rescue
- Do not change `semantic_rescue_threshold`
- Do not change container rescue, anchor split, or bucketing
- Do not remove or modify the `[diag]` prints
- Do not change any types in `FileCommunityConfig` or `PlannerDiagnostics`

### Tests

Add or update these tests:

1. **`semantic_rescue_does_not_bridge_components`**
   Create two disconnected components (A with 3 symbols, B with 3 symbols)
   and a degree-0 orphan with embeddings similar to symbols in both A and B.
   After rescue: the orphan joins ONE component (the one with the best match).
   Components A and B remain disconnected.

2. **`semantic_rescue_densifies_within_component`**
   Create one component with 5 symbols, one of which has degree 1 and high
   similarity to another in the same component. After rescue: new edge added
   within the component. Component count unchanged.

3. **`semantic_rescue_orphan_becomes_leaf`**
   A singleton orphan with a good semantic match. After rescue: orphan has
   exactly degree 1 (leaf node). It joins the target's component.

4. **`semantic_rescue_orphan_prefers_existing_component`**
   Create two non-singleton components and one singleton orphan. The orphan has
   similar embeddings to symbols in both components but slightly better match to
   component A. After rescue: orphan joins component A, not a singleton pair.

5. **`semantic_rescue_stability_under_threshold_perturbation`**
   Run rescue with threshold T and T+0.05. The community assignments should have
   Jaccard similarity >= 0.90 (stability check).

6. **Existing test updates:** `semantic_rescue_connects_isolated_symbols`,
   `semantic_rescue_skips_high_degree_symbols`, `semantic_rescue_respects_top_k`
   — these must still pass. If they construct scenarios that assume cross-component
   rescue, adjust them to test within-component behavior instead.

## VALIDATION GATE

```bash
cargo fmt --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
```

Then run ablation for all three crates:

```bash
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
# Must say "sqlite"
rm -f /home/rephu/projects/aether/.aether/graph/LOCK

cargo test -p aether-health -- ablation_aether_store --ignored --nocapture 2>&1
cargo test -p aether-health -- ablation_aether_mcp --ignored --nocapture 2>&1
cargo test -p aether-health -- ablation_aether_config --ignored --nocapture 2>&1
```

### Ablation pass criteria

Check the `[diag]` output for row 4 (+ rescue) and row 6 (full pipeline):

1. **Per-pass 30% rule:** In EACH of the 3 internal stability passes,
   `after_semantic_rescue` must not reduce component count by more than 30%
   from that same pass's `after_container_rescue`. (Currently the baseline pass
   drops 80%: 45 → 9. Target: each pass stays at 45 → 32 or better.)

2. All 3 internal stability passes should produce similar `after_semantic_rescue`
   component counts (not wildly different like 9 vs 33).

3. Row 6 (full pipeline): communities >= 10, largest < 100, stability >= 0.90.

4. **aether-mcp and aether-config ablations must not regress materially.** If
   aether-store improves but either of the others regresses, STOP and report
   before committing.

5. If the ablation improves but doesn't fully meet criteria, commit anyway and
   print the full output. The `[diag]` data determines the next move.

## COMMIT

```bash
git add -A
git commit -m "Constrain semantic rescue to component-bounded edges

- Compute connected components before semantic rescue runs
- Within-component symbols: rescue only within same component (local
  densification, no cross-component bridges)
- Singleton orphans: absorb into best non-singleton component with
  take(1), creating a leaf node that cannot bridge components
- Dynamic re-check prevents multi-symbol rep spray
- Never add self-edges or duplicate edges
- Fixes instability where 0.05 threshold shift changed component
  count from 9 to 33 due to cross-component bridging"
```

Do NOT push. Robert will review the ablation output first.
