# Codex Prompt — Phase 8.12.2 Continuation: Diagnostics + First-Token Bucketing

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

You are working in an EXISTING worktree on an EXISTING branch. Do NOT create a new branch
or worktree. This is a continuation of commit `01c2ab2`, not a fresh start.

**IMPORTANT: If the diagnostics show that the community collapse already happens
immediately after `collapse_structural_edges` (before any rescue runs), report that
explicitly in your output before making any further rescue-stage changes. The diagnostics
are the primary deliverable — the bucketing fix is secondary.**

Read this file before writing any code:
- `crates/aether-health/src/planner_communities.rs` (THE ONLY FILE TO MODIFY)

Read the companion session context document for full background:
- `docs/hardening/phase8_12_2_codex_session_context.md`

## PREFLIGHT

```bash
cd /home/rephu/projects/aether-phase8-anchor-split
git status --porcelain
# Must be clean. If dirty, STOP and report.
git log --oneline -1
# Should show 01c2ab2. If different, STOP and report.
```

Check vector_backend before any ablation run:
```bash
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
# MUST say "sqlite". If "lancedb", fix:
# sed -i 's/vector_backend = "lancedb"/vector_backend = "sqlite"/' /home/rephu/projects/aether/.aether/config.toml
```

Remove stale graph lock if present:
```bash
rm -f /home/rephu/projects/aether/.aether/graph/LOCK
```

## TASK 1: Add per-step diagnostics to `run_ablation_pass`

In the `run_ablation_pass` function (inside `mod tests`), add diagnostic `eprintln!`
statements after each pipeline step. These print to stderr so they appear in
`--nocapture` output without polluting the table formatting.

After each step, compute and print connected component count and largest component
size using the current graph state. Use this helper (add it inside `mod tests`):

```rust
fn count_components_and_largest(
    graph: &WeightedGraph,
    rep_to_members: &[Vec<usize>],
    entries: &[SymbolEntry],
) -> (usize, usize) {
    let active_reps: Vec<usize> = rep_to_members
        .iter()
        .enumerate()
        .filter_map(|(rep, members)| {
            if members.is_empty() {
                None
            } else {
                Some(rep)
            }
        })
        .collect();
    if active_reps.is_empty() {
        return (0, 0);
    }
    let components = graph.connected_components(&active_reps, entries);
    let largest = components
        .iter()
        .map(|c| c.iter().map(|rep| rep_to_members.get(*rep).map(Vec::len).unwrap_or(0)).sum::<usize>())
        .max()
        .unwrap_or(0);
    (components.len(), largest)
}
```

Insert `eprintln!` calls at these points in `run_ablation_pass`:

1. **After anchor groups + split + rebuild** (after `build_anchor_groups` or
   `split_large_anchor_groups` + `rebuild_union_find_from_groups`, before
   `collapse_structural_edges`):
   ```rust
   let rep_to_members_diag = build_rep_members(entries.len(), &mut union_find);
   let (nc, nl) = count_components_and_largest(&WeightedGraph::default(), &rep_to_members_diag, &entries);
   eprintln!("[diag] after_anchor_split: groups={} largest_group={}", nc, nl);
   ```
   Note: Use an empty graph here because structural edges haven't been added yet.
   "groups" = number of non-empty union-find groups. "largest_group" = member count
   of the largest group.

2. **After `collapse_structural_edges`** (after the structural_graph is built and
   enriched_graph is cloned, after `build_rep_members` and `rep_by_index` are
   recomputed):
   ```rust
   let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
   eprintln!("[diag] after_structural_edges: components={} largest_component={}", nc, nl);
   ```

3. **After `apply_container_rescue_with_exclusions`**:
   ```rust
   let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
   eprintln!("[diag] after_container_rescue: components={} largest_component={} rescued={}", nc, nl, symbols_rescued_container);
   ```

4. **After `apply_semantic_rescue`**:
   ```rust
   let (nc, nl) = count_components_and_largest(&enriched_graph, &rep_to_members, &entries);
   eprintln!("[diag] after_semantic_rescue: components={} largest_component={} rescued={}", nc, nl, symbols_rescued_semantic);
   ```

5. **After connected_components** (after the `components` variable is computed):
   ```rust
   let component_sizes: Vec<usize> = components.iter().map(Vec::len).collect();
   eprintln!("[diag] connected_components: count={} sizes={:?}", components.len(), component_sizes);
   ```

6. **After Louvain** (after `communities_before_merge` is computed):
   ```rust
   eprintln!("[diag] after_louvain: communities={}", communities_before_merge);
   ```

These 6 prints go ONLY in `run_ablation_pass` (inside `mod tests`), NOT in the
production `run_detection` function.

## TASK 2: Replace rarest-token bucketing with first-token bucketing

Modify the `split_large_anchor_groups` function to use the FIRST informative token
from each method name as the bucket key, instead of the rarest token.

### Algorithm for bucket key extraction:

For a qualified_name like `SqliteStore::upsert_sir_meta`:
1. Extract the method name: take the part after the last `::` → `upsert_sir_meta`
2. Split by `_` → `["upsert", "sir", "meta"]`
3. Filter out tokens in the ANCHOR_STOPWORDS list
4. Take the FIRST remaining token → `sir`
5. If no informative token remains, use `"misc"`

The bucket key is this first informative token. All methods whose first informative
token is `sir` go into the `sir` bucket, all whose first is `project` go into `project`,
etc.

### Expected bucket distribution for aether-store:

This should produce roughly 8-12 medium-sized domain buckets on aether-store, with
bucket keys like `sir`, `project`, `drift`, `intent`, `symbol`, `edge`, `embedding`,
`community`, `migration`, etc. Do not try to match specific member counts per bucket —
the goal is domain-aligned grouping, not a particular size distribution.

### Keep unchanged:

- `ANCHOR_SPLIT_THRESHOLD` (20) — only split groups larger than this
- `ANCHOR_MIN_BUCKET` (3) — buckets smaller than this get merged
- `ANCHOR_STOPWORDS` — the existing stopword list
- The small-bucket merge logic (absorb tiny buckets into nearest large bucket)
- `rebuild_union_find_from_groups` — no changes needed
- `apply_container_rescue_with_exclusions` — no changes needed
- The exclusion set logic in both `run_detection` and `run_ablation_pass`
- All 6 tests from commit 01c2ab2 must still pass (may need minor adjustments
  if they test specific bucket assignments that change with first-token logic)

### Modify:

- `split_large_anchor_groups`: Replace the bucket-key extraction logic. Remove
  `informative_compound`, `token_overlap`, and any rarest-token scoring. Replace
  with the first-informative-token algorithm above.
- If `informative_tokens` exists and is used elsewhere, keep it. If it's only used
  for rarest-token logic, remove it.
- `normalize_anchor_token` can stay if useful. If not used, remove dead code.

### Stopword list guidance:

The existing `ANCHOR_STOPWORDS` should include common Rust/storage verbs that don't
indicate domain:
```
"get", "set", "list", "insert", "upsert", "delete", "remove", "update", "create",
"open", "close", "load", "save", "write", "read", "store", "resolve", "search",
"find", "query", "count", "check", "ensure", "build", "parse", "format",
"new", "from", "into", "with", "default", "init",
"for", "by", "all", "the", "and", "or", "is", "has", "can", "do",
"fn", "test", "mock", "impl", "try", "run", "record", "acknowledge"
```

This list should be reviewed against the actual method names in aether-store. If a token
like `record` or `acknowledge` acts as a bucket key that only captures 1-2 methods and
creates noise, add it to stopwords. If it captures a meaningful domain group, keep it
as informative.

**Do not aggressively expand stopwords in this phase.** Prefer the minimal edits needed
to reproduce prototype-like grouping. Over-expanding stopwords is how you end up with
a different kind of bucket collapse.

## TASK 3: Validation gate

After both changes:

```bash
cargo fmt --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
```

Then run the ablation:

```bash
# Ensure vector_backend = "sqlite" and no LOCK file
grep vector_backend /home/rephu/projects/aether/.aether/config.toml
rm -f /home/rephu/projects/aether/.aether/graph/LOCK

cargo test -p aether-health -- ablation_aether_store --ignored --nocapture 2>&1
```

The diagnostic lines (starting with `[diag]`) will appear in stderr output.
The ablation table will appear in stdout.

### Pass criteria:

1. All `cargo test -p aether-health` tests pass (including the 6 from 01c2ab2)
2. Zero clippy warnings
3. Ablation `[diag]` lines print for all 6 rows
4. Ablation row 6 (full pipeline) shows 5-16 communities, largest < 100, 0 loners

### If ablation does not meet criteria:

If the first-token bucketing improves things but doesn't hit the target, DO NOT revert.
Commit the diagnostics + bucketing fix as-is. The diagnostics output will tell us which
step is still causing collapse, and we can fix it in a follow-up.

Print the full ablation table AND all `[diag]` lines in your output so the human can
see exactly where the remaining problem is.

## COMMIT

If validation passes:

```bash
git add -A
git commit -m "Add per-step diagnostics and first-token bucketing for anchor split

- Add [diag] prints to run_ablation_pass showing component count and
  largest component after each pipeline step (anchor, structural edges,
  container rescue, semantic rescue, connected components, Louvain)
- Replace rarest-token bucketing with first-token bucketing in
  split_large_anchor_groups: use first non-stopword token from method
  name as bucket key, matching the prototype approach that achieved
  16 communities / 92 largest / 0 loners
- Update stopword list for domain-agnostic storage verbs
- All existing tests pass, zero clippy warnings"
```

Do NOT push. Robert will review the ablation output first.
