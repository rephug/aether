# Codex Prompt — Phase 8.21a: Two-Signal Trait Clustering

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read the spec and session context first:
- `docs/roadmap/phase_8_stage_8_21a_two_signal_clustering.md`
- `docs/hardening/phase8_stage8_21a_session_context.md`

Then read:
- `crates/aether-health/src/planner.rs` — THE ONLY FILE TO MODIFY

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -b fix/trait-cluster-two-signal /home/rephu/aether-fix-trait-cluster
cd /home/rephu/aether-fix-trait-cluster
```

## SOURCE INSPECTION

Before writing code, read `suggest_trait_split` in planner.rs and verify:

1. The function takes `method_dependencies: Option<&HashMap<String, Vec<String>>>`.
2. Currently, method_dependencies is used ONLY for naming (via
   `dominant_dependencies_for_cluster`), NOT for clustering.
3. `merge_similar_trait_clusters` only merges singletons with Jaccard >= 0.80
   on consumer sets. It does NOT use dependency data.
4. `method_dependency_values` resolves a method's dependencies from the
   HashMap by trying multiple key formats (name, display_name, qualified_name, leaf).
5. `display_dependency_name` strips generics and path prefixes from type names.
6. `jaccard_similarity` takes two `HashSet<usize>` and returns f32.
7. Existing tests pass `method_dependencies: None`.

## IMPLEMENTATION

### Step 1: Add dependency set builder

Add a function that builds filtered dependency sets per method:

```rust
fn build_filtered_dep_sets(
    methods: &[TraitMethod],
    method_dependencies: Option<&HashMap<String, Vec<String>>>,
    ubiquity_threshold: f32,
) -> (Vec<HashSet<String>>, bool)
// Returns: (dep_sets_per_method, has_sufficient_data)
// has_sufficient_data = true when >= 50% of methods have non-empty dep sets
```

Logic:
1. For each method, get its dependencies via `method_dependency_values`
2. Normalize each dependency via `display_dependency_name`
3. Count frequency of each dependency type across ALL methods
4. Build a ubiquitous-type stoplist: types appearing in more than
   `ubiquity_threshold` fraction of methods (default 0.80)
5. Rebuild each method's dep set with stoplist types removed
6. Count how many methods have non-empty dep sets after filtering
7. Return (filtered dep sets, count >= methods.len() / 2)

### Step 2: Add fused similarity function

```rust
fn fused_method_similarity(
    dep_set_a: &HashSet<String>,
    dep_set_b: &HashSet<String>,
    consumer_set_a: &HashSet<usize>,
    consumer_set_b: &HashSet<usize>,
) -> f32
```

Logic:
- If BOTH dep sets are empty (utility fallback): return `jaccard_similarity_generic(consumer_set_a, consumer_set_b)`
- Otherwise: `0.75 * jaccard_similarity_generic(dep_set_a, dep_set_b) + 0.25 * jaccard_similarity_generic(consumer_set_a, consumer_set_b)`

You'll need a generic Jaccard that works on `HashSet<String>` as well
as `HashSet<usize>`. Either make a generic version or add a string variant.
The existing `jaccard_similarity` only takes `HashSet<usize>`.

### Step 3: Add agglomerative clustering

```rust
fn agglomerative_trait_clusters(
    called_method_indices: &[usize],
    method_consumers: &[HashSet<usize>],
    method_dep_sets: &[HashSet<String>],
    merge_threshold: f32,  // 0.30
) -> Vec<Vec<usize>>
```

Logic:
1. Initialize: one cluster per called method index
2. Loop:
   a. For every pair of clusters, compute average fused similarity:
      - For each (method_a in cluster_x, method_b in cluster_y),
        compute `fused_method_similarity`
      - Average across all pairs
   b. Find the pair with the highest average score
   c. If highest score < merge_threshold, stop
   d. Merge the two clusters
3. Sort final clusters deterministically (by first method name)
4. Return

Performance note: For N=59 methods this is O(N^3) per merge step,
which is trivially fast. No optimization needed.

### Step 4: Wire into suggest_trait_split

In `suggest_trait_split`, replace the current clustering section:

**Current flow:**
```
clusters_by_consumers (exact match) → merge_similar_trait_clusters (singleton Jaccard 0.80)
```

**New flow:**
```
build_filtered_dep_sets
  → if has_sufficient_data:
      agglomerative_trait_clusters (fused 0.75/0.25, threshold 0.30)
  → else:
      clusters_by_consumers + merge_similar_trait_clusters (existing fallback)
      set confidence to Low with note
```

After the clustering step, everything else stays the same: consumer
isolation, cross-cutting detection, naming, confidence scoring.

When falling back to consumer-only (insufficient dep data), force
confidence to Low even if isolation scores are good, and append to
the first suggested_trait's reason: "Note: dependency data unavailable
for most methods; clustering based on consumer patterns only."

### Step 5: Update cross-cutting reason text

When dep data IS available, update the cross-cutting reason to:
"Structurally belongs in {home_cluster} due to shared types, but usage
heavily overlaps with {overlapping_clusters}"

When dep data is NOT available (fallback), keep the existing reason:
"Consumers overlap {N} clusters"

## TESTS

### New tests to add:

1. **Two-signal clustering with deps:** 4 methods. Methods 1+2 share
   dep type "Record" but have different consumers. Methods 3+4 share
   dep type "Config" but have different consumers. Verify 2 clusters
   formed by dep similarity despite consumer divergence.

2. **Ubiquitous type filtering:** 4 methods. All share "StoreError" as
   a dep. Methods 1+2 additionally share "SirMeta". Methods 3+4
   additionally share "NoteRecord". Verify "StoreError" is filtered out
   and 2 clusters form around "SirMeta" and "NoteRecord".

3. **Utility fallback:** 4 methods with NO dep data at all. Methods 1+2
   share consumers. Methods 3+4 share consumers. Verify consumer-only
   clustering still works (same as current behavior).

4. **Graceful degradation:** Pass method_dependencies with only 1 out
   of 4 methods having data (<50%). Verify fallback to consumer-only
   with Low confidence.

5. **Fused score math:** Unit test for `fused_method_similarity` with
   known dep and consumer sets. Verify exact score matches expected
   0.75 * dep + 0.25 * consumer formula.

### Existing tests must still pass:

The existing `suggest_trait_split` tests use `method_dependencies: None`
which triggers the degradation fallback path. They should produce the
same clusters as before (consumer-only). If their confidence changes
from Medium/High to Low (because of the forced Low on degradation),
update the assertions to expect Low.

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
```

Then check no downstream breakage:
```bash
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

## COMMIT

```bash
git add -A
git commit -m "Replace consumer-only trait clustering with two-signal agglomerative algorithm (0.75 dep + 0.25 consumer)"
```

Do NOT push automatically. Report commit SHA and wait for review.

## PR Description

**Title:** Phase 8.21a: Two-signal agglomerative clustering for trait split planner

**Body:**

Replaces the consumer-bitvector exact-match clustering in `suggest_trait_split` with an agglomerative hierarchical algorithm that fuses two signals: dependency-type similarity (0.75 weight, from SIR `method_dependencies`) and consumer co-usage patterns (0.25 weight, from CALLS edges).

**Why:** Running `aether_suggest_trait_split` on Store (59 methods, 65 consumers) produced "low" confidence with mostly singleton clusters because consumer fingerprints are too unique. Three manual AI-produced plans independently converged on 11 sub-traits using domain reasoning (shared types), not consumer patterns. This patch makes the algorithm match expert behavior.

**Algorithm:** Average-linkage agglomerative clustering with fused pairwise scores. Merging stops when the best remaining score drops below 0.30. Ubiquitous dependency types (appearing in >80% of methods) are filtered before scoring. When <50% of methods have dependency data, gracefully degrades to consumer-only clustering with Low confidence.

**Design credit:** Algorithm design from Deepthink analysis of the Store trait decomposition experiment results.

**Depends on:** 8.20 (method_dependencies in SIR), 8.21 (trait split planner infrastructure)
