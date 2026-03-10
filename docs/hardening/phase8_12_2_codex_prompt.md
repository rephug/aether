# Phase 8.12.2 — Large Anchor Intra-Partitioning — Codex Prompt

## Preflight

```bash
git status --porcelain
# Must be clean. If not, stop and report dirty files.
git pull --ff-only
```

## Branch and worktree

```bash
git checkout -b feature/phase8-anchor-split
git worktree add ../aether-phase8-anchor-split feature/phase8-anchor-split
cd ../aether-phase8-anchor-split
```

## Build environment (use for ALL cargo commands)

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

## Context — read these files first

```
crates/aether-health/src/planner_communities.rs  — ENTIRE file. This is the only file being modified.
crates/aether-health/src/planner.rs              — naming logic (read-only, for reference)
```

## Critical rule

**If the actual source layout or type signatures differ from this prompt, follow the
source, not the prompt. Stop and report the mismatch.**

## Scope guard — do NOT modify

- Health scoring formulas
- Existing CLI commands, API endpoints, MCP tools, LSP hover
- Store traits or implementations
- Planner naming logic in planner.rs
- Global community snapshot, coupling mining, drift detection, dashboard
- Edge extraction in aether-parse
- Small anchor group behavior (groups with <= ANCHOR_SPLIT_THRESHOLD members must
  produce IDENTICAL results to the current code)

## Design principle

One file, one behavioral change: large anchor groups get split into domain-based
sub-groups before Louvain runs. Everything else stays the same. No new crate
dependencies. No new public API. No config changes in this stage (threshold is
a const, tunable later).

## Background — the proven problem

Ablation testing on aether-store/src/lib.rs (5869 LOC, ~214 non-test symbols) showed:

```
Isolated rescue comparison:
  type-anchor only:      7 communities, 141 largest, 48 loners
  + container only:      2 communities, 155 largest,  0 loners  ← COLLAPSE
  + semantic only:      21 communities, 116 largest,  6 loners  ← WORKS
  both rescues:          2 communities, 155 largest,  0 loners
```

Root cause: `build_anchor_groups` unions ALL 141 SqliteStore methods into one
super-node. Container rescue then bridges remaining fragments via shared
`SqliteStore::` stem. Result: 2 blobs.

A prototype of the fix produced:
```
  type-anchor:    8 communities, 114 largest, 63 loners
  + rescue:      17 communities,  90 largest,  0 loners
  full pipeline: 16 communities,  92 largest,  0 loners
```

All 52 existing unit tests passed with the prototype.

---

## Implementation

All changes are in `crates/aether-health/src/planner_communities.rs`.

### Step 1: Add constants and helper functions

Add these BEFORE `build_anchor_groups`:

**Constants:**
```rust
const ANCHOR_SPLIT_THRESHOLD: usize = 20;
const ANCHOR_MIN_BUCKET: usize = 3;
```

**ANCHOR_STOPWORDS** — CRUD verbs and grammar words that don't indicate domain:
```
get, set, list, find, read, write, upsert, delete, remove, insert, update,
create, mark, clear, prune, count, increment, record, load, save, open, close,
new, default, from, into, with, for, the, and, is, has, all, batch, by, if, or,
run, do, try, check, ensure, resolve, as, to, sync
```

**`normalize_anchor_token(token: &str) -> String`** — lowercase, strip `r#` prefix,
normalize plurals. Explicit match arms for common domain terms:
`note/notes`, `project/projects`, `migration/migrate/migrations`,
`embedding/embeddings`, `symbol/symbols`, `intent/intents`, `store/stores`,
`edge/edges`, `version/versions`, `request/requests`, `result/results`,
`graph/graphs`, `schema/schemas`, `module/modules`, `provider/providers`,
`model/models`, `meta/metas`, `history/histories`.
Fallback: strip trailing `s` if len > 3.

**`informative_tokens(name: &str) -> Vec<String>`** — take leaf name (after last `::`),
split by `_`, normalize each token, filter out tokens with len <= 1 and stopwords.

**`informative_compound(name: &str) -> Option<String>`** — if informative_tokens
produces >= 2 tokens, return `"{first}_{second}"`. Else None. This captures
domain pairs like `project_note`, `sir_meta`, `provider_model`.

**`token_overlap(left: &[String], right: &[String]) -> usize`** — count of tokens
in `left` that also appear in `right`. Use a HashSet for the right side.

### Step 2: Add `split_large_anchor_groups`

```rust
fn split_large_anchor_groups(
    entries: &[SymbolEntry],
    original_groups: Vec<Vec<usize>>,
) -> (Vec<Vec<usize>>, HashSet<usize>)
```

Returns: (new groups, set of symbol indices that were part of a split anchor).

Algorithm:
1. For each group in original_groups:
   - If group.len() <= ANCHOR_SPLIT_THRESHOLD → pass through unchanged.
   - If no type anchor in group → pass through unchanged.
   - Separate anchor_indices (struct/enum/trait/type_alias) from method_indices.
   - If method_indices.len() < ANCHOR_SPLIT_THRESHOLD → pass through unchanged.

2. **Build token frequency map** across all method names in the group.

3. **Assign each method to a bucket** by its most specific (rarest) informative token:
   - First check: does this method have a compound token (e.g. `project_note`) that
     is shared by at least one other method in the group? If yes, use the compound
     as bucket key. This keeps `upsert_project_note` and `list_project_notes` together.
   - Otherwise: pick the rarest informative token that appears in <= 50% of methods.
     Rarest = lowest frequency in the token_freq map.
   - Fallback if all tokens are common (>50%): pick rarest overall.
   - Fallback if no informative tokens: bucket key = "misc".
   - Tie-break: lexicographic order (deterministic).

4. **Merge small buckets** (< ANCHOR_MIN_BUCKET members) into the nearest large bucket
   by token overlap. Tie-break: largest bucket size, then lexicographic key.
   If no large buckets exist after filtering, pass through the group unchanged.

5. If only 1 bucket remains after merging → pass through unchanged.

6. **Record all member indices** of this group into the `split_members` HashSet.

7. **Attach type anchor indices** to the largest bucket (by member count, then
   lexicographic key tie-break). Type anchors are NOT bucketed by their own name.

8. Sort buckets by key, sort members within each bucket. Emit as separate groups.

### Step 3: Add `rebuild_union_find_from_groups`

```rust
fn rebuild_union_find_from_groups(num_entries: usize, groups: &[Vec<usize>]) -> DisjointSet
```

Build a fresh DisjointSet. For each group with 2+ members, union all members
to the first member. Do NOT mutate the original union-find — build from scratch.

### Step 4: Add `apply_container_rescue_with_exclusions`

```rust
fn apply_container_rescue_with_exclusions(
    entries: &[SymbolEntry],
    rep_by_index: &[usize],
    rep_to_members: &[Vec<usize>],
    graph: &mut WeightedGraph,
    split_exclusions: &HashSet<usize>,
) -> usize
```

This is the existing `apply_container_rescue` logic with one addition: skip any
singleton rep whose member index is in `split_exclusions`. This prevents container
rescue from re-merging deliberately split anchor sub-groups via shared stems.

**Keep the existing `apply_container_rescue` function as a wrapper** that calls
`apply_container_rescue_with_exclusions` with an empty HashSet. This preserves
backward compatibility for existing unit tests that call `apply_container_rescue`
directly.

Mark the wrapper with `#[allow(dead_code)]` if the main pipeline no longer calls
it directly (it may still be called by unit tests).

### Step 5: Patch `run_detection` (main pipeline)

Find the line (approximately line 331):
```rust
let (mut union_find, rep_to_members) = build_anchor_groups(entries.as_slice());
```

Replace with:
```rust
let (_anchor_union_find, anchor_groups) = build_anchor_groups(entries.as_slice());
let (rep_to_members, split_anchor_exclusions) =
    split_large_anchor_groups(entries.as_slice(), anchor_groups);
let mut union_find =
    rebuild_union_find_from_groups(entries.len(), rep_to_members.as_slice());
```

Find the `apply_container_rescue(` call in `run_detection` and replace with
`apply_container_rescue_with_exclusions(` passing `&split_anchor_exclusions`.

### Step 6: Patch `run_ablation_pass`

The ablation path has a different pattern — it's inside an `if options.type_anchor`
block. Read the actual code carefully. The pattern is approximately:

```rust
let (mut union_find, initial_rep_to_members) = if options.type_anchor {
    build_anchor_groups(entries.as_slice())
} else {
    ...
};
```

Replace the type_anchor branch to also split large anchors and produce
`split_anchor_exclusions`. The else branch produces an empty `HashSet::new()`.

Find the `apply_container_rescue(` call in `run_ablation_pass` (inside
`if options.container_rescue { ... }`) and replace with
`apply_container_rescue_with_exclusions(` passing `&split_anchor_exclusions`.

**Important:** The functions `split_large_anchor_groups`,
`rebuild_union_find_from_groups`, and `apply_container_rescue_with_exclusions`
are private module-level functions. The ablation code lives inside `mod tests`.
Use `super::split_large_anchor_groups` etc. to call them from the test module,
or keep them as module-level private functions and call via full path
`crate::planner_communities::split_large_anchor_groups` — whichever compiles
cleanly without `pub(crate)` warnings on private types like `SymbolEntry`,
`DisjointSet`, `WeightedGraph`.

The cleanest approach: keep all new functions private (no pub/pub(crate)),
and call them from the test module via `super::` since the test module is
`mod tests` inside `planner_communities.rs`.

### Step 7: Add tests

Add these tests in the existing `mod tests` block:

**`split_large_anchor_skips_small_groups`**
Build a group of 10 members with a type anchor. Verify split_large_anchor_groups
returns it unchanged. Verify split_members is empty.

**`split_large_anchor_partitions_by_domain_token`**
Build a group of 30+ members: 1 struct `SqliteStore`, plus methods named
`upsert_sir_meta`, `list_sir_history`, `get_sir_version`,
`upsert_project_note`, `list_project_notes`, `delete_project_note`,
`run_migrations`, `migration_v6_renames`, `migration_from_v2`,
`create_write_intent`, `update_intent_status`, `mark_intent_failed`,
plus enough additional methods to exceed threshold.
Verify: produces multiple groups. The `sir_*` methods are in one group.
The `project_note_*` methods are in one group. The `migration_*` methods
are in one group. The struct `SqliteStore` is in the largest group.

**`split_large_anchor_type_not_singleton`**
Verify the type anchor (struct) does NOT end up in its own singleton bucket.
It must be in a group with other members.

**`split_large_anchor_preserves_small_anchor_behavior`**
Build a small anchor group (struct + 5 methods). Run the full
`detect_file_communities` pipeline. Verify the struct and its methods
land in the same community — identical to pre-patch behavior.

**`container_rescue_skips_split_anchor_members`**
Build a scenario where split-anchor members would be rescued by container
rescue if not excluded. Verify they are NOT rescued (split_exclusions works).

**`container_rescue_still_works_for_non_split_symbols`**
Verify that symbols NOT in split_exclusions are still rescued normally
by container rescue. The wrapper `apply_container_rescue` (empty exclusions)
still works for existing tests.

### Step 8: Validation

```bash
cargo fmt --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
```

Do NOT run `cargo test --workspace` — OOM risk.

Zero warnings required. The `pub(crate)` warnings from the prototype must not
appear — use private functions with `super::` calls from tests.

### Step 9: Commit

```bash
git add -A
git commit -m "Split large anchor groups for community detection quality"
```

---

## Post-implementation verification

```bash
# Run ablation to verify improvement
cargo test -p aether-health -- ablation_aether_store --ignored --nocapture

# Expected: ~16 communities for full pipeline (was 2 before this change)
# Expected: largest community ~90 (was 155)
# Expected: 0 loners
# Expected: module names showing domain tokens (sir_ops, graph_ops, etc.)

# Also run aether-mcp ablation to check for regressions
cargo test -p aether-health -- ablation_aether_mcp --ignored --nocapture
```

---

## Summary of what this changes

1. **New step in pipeline:** after `build_anchor_groups`, large groups (> 20 members)
   are split into sub-groups by domain-token affinity. Small groups are untouched.

2. **Type anchors attach to largest bucket**, not bucketed by their own name. Prevents
   `SqliteStore` becoming a useless singleton.

3. **Container rescue exclusions:** split-anchor members are excluded from container
   rescue, preventing re-merging via shared stems.

4. **Both pipeline paths patched:** `run_detection` (production) and `run_ablation_pass`
   (testing) use the same split + exclusion logic.

5. **DSU rebuilt from final groups**, not mutated in place. Clean and safe.

6. **Policy change documented:** type-anchor is hard for small impls, soft for large
   service structs. This matches what a human architect would do — `SqliteStore` is
   a namespace of unrelated storage domains, not a true cohesive module.

## What this does NOT change

- Small anchor groups (< threshold) — identical behavior
- Semantic rescue — untouched
- Louvain algorithm — untouched
- Naming logic — untouched
- Health scoring — untouched
- Config schema — no new fields (threshold is a const)
- Public API — no changes
