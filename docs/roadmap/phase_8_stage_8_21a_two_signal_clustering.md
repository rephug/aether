# Phase 8.21a: Two-Signal Trait Clustering

## Purpose

Replace the consumer-only clustering in `suggest_trait_split` with a
two-signal agglomerative algorithm that fuses dependency-type similarity
(from SIR `method_dependencies`) with consumer co-usage patterns. This
addresses the "low confidence / singleton explosion" problem observed
when running `aether_suggest_trait_split` on the 59-method Store trait.

## Problem

The current algorithm groups methods by identical consumer sets, then
merges singletons via Jaccard >= 0.80 on consumer bitvectors. On Store
(59 methods, 65 consumers), this produces dozens of singleton clusters
and "low" confidence because consumer fingerprints are too unique — each
method has a nearly unique set of callers.

Manual decomposition by three AI agents independently converged on 11
sub-traits using **domain reasoning** ("these methods all operate on SIR
types") not consumer patterns alone. The dependency-type signal needs to
be the primary clustering driver, with consumer patterns as validation.

## Design (from Deepthink analysis)

### Pairwise scoring

For every pair of methods, compute a fused similarity score:

```
Score(A, B) = (0.75 × dep_jaccard) + (0.25 × consumer_jaccard)
```

Where:
- `dep_jaccard` = Jaccard similarity of filtered dependency type sets
- `consumer_jaccard` = Jaccard similarity of consumer file sets (existing)

**Ubiquitous type filtering:** Before computing `dep_jaccard`, strip
types that appear in >80% of methods. These are noise (`StoreError`,
`String`, `Option`, `Result`, `Vec`). Compute the frequency of each
dependency type across all methods, build a stoplist of types exceeding
the 80% threshold, and exclude them from all dependency sets.

**Utility fallback:** If BOTH methods in a pair have zero domain
dependencies after filtering (pure utility methods), fall back to
consumer-only: `Score(A, B) = consumer_jaccard`.

### Clustering algorithm

Replace the current exact-match + singleton-merge approach with
**Agglomerative Hierarchical Clustering (Average Linkage)**:

1. Initialize: each called method is its own cluster
2. Compute average fused score between all cluster pairs
3. Merge the pair with the highest average score
4. Repeat until the highest remaining score < 0.30
5. Uncalled methods (zero consumers) skip clustering entirely,
   reported in `uncalled_methods` as before

**Why 0.30:** Two methods sharing half their domain types (Jaccard=0.5)
and zero consumers score 0.375, which clears the threshold. Methods
with zero shared types need impossible consumer overlap to merge. This
prevents both over-merging and singleton explosion.

**Why average linkage:** Single linkage chains unrelated methods through
one shared type. Complete linkage refuses to merge clusters where any
pair is dissimilar. Average linkage balances both.

### Cross-cutting detection

After clustering, for each method, check if its consumer set has
`significant_overlap_ratio >= 0.50` with any cluster it was NOT placed
in. If it overlaps 2+ other clusters, flag it as cross-cutting with
reason: "Structurally belongs in {home_cluster} due to shared types,
but usage heavily overlaps with {other_clusters}."

### Graceful degradation

When `method_dependencies` is `None` or fewer than 50% of methods have
dependency data, fall back to consumer-only clustering (the current
behavior with Jaccard >= 0.80). Emit the confidence as Low with a note:
"Dependency data unavailable for most methods; clustering based on
consumer patterns only."

### Naming (unchanged)

The existing `dominant_dependencies_for_cluster` and
`ranked_identifier_tokens` functions already produce good names from
dependency types. No change needed — the new clusters will have better
dependency coherence, so names will be more accurate automatically.

## What Changes in planner.rs

### Replace `merge_similar_trait_clusters`

The current function only merges singletons with Jaccard >= 0.80 on
consumer sets. Replace it with the full agglomerative algorithm.

New function: `agglomerative_trait_clusters`

```rust
fn agglomerative_trait_clusters(
    method_consumers: &[HashSet<usize>],
    method_deps: &[HashSet<String>],  // filtered dep sets per method
    called_methods: &[usize],          // indices of methods with >0 consumers
) -> Vec<Vec<usize>>
```

### Replace clustering section in `suggest_trait_split`

Currently:
1. Group by exact consumer set → `clusters_by_consumers`
2. Call `merge_similar_trait_clusters` for singleton absorption

Replace with:
1. Build filtered dependency sets per method
2. If <50% have deps, fall back to current consumer-only path
3. Otherwise, call `agglomerative_trait_clusters`

### Add dependency filtering helpers

```rust
fn build_filtered_dep_sets(
    methods: &[TraitMethod],
    method_dependencies: Option<&HashMap<String, Vec<String>>>,
    ubiquity_threshold: f32,  // 0.80
) -> Vec<HashSet<String>>
```

Returns one `HashSet<String>` per method with ubiquitous types removed.
Methods with no dependency data get an empty set.

```rust
fn fused_similarity(
    dep_set_a: &HashSet<String>,
    dep_set_b: &HashSet<String>,
    consumer_set_a: &HashSet<usize>,
    consumer_set_b: &HashSet<usize>,
) -> f32
```

Returns the weighted score. Handles the utility fallback internally.

### Keep everything else

Consumer isolation, cross-cutting detection, naming, confidence scoring,
and output formatting all stay the same. Only the clustering step changes.

## Files to Modify

| File | Change |
|------|--------|
| `crates/aether-health/src/planner.rs` | Replace clustering logic, add agglomerative algorithm + dep filtering |

No other files change. The MCP tool and CLI consume `suggest_trait_split`
output which keeps the same return type. The change is internal to the
clustering algorithm.

## Pass Criteria

1. `suggest_trait_split` on Store (59 methods, 65 consumers, with
   method_dependencies populated) produces 8-12 clusters instead of
   dozens of singletons.
2. Confidence is Medium or High, not Low.
3. Clusters roughly correspond to the manual 11-subtrait decomposition
   (SymbolCatalog, SymbolRelation, SirState, SirHistory, SemanticIndex,
   Threshold, ProjectNote, ProjectNoteEmbedding, CouplingState, Drift,
   TestIntent).
4. `CouplingStateStore` methods (`get_coupling_mining_state`,
   `upsert_coupling_mining_state`) cluster together.
5. Cross-cutting methods (e.g., `mark_removed`) are flagged.
6. When method_dependencies is None, falls back to consumer-only with
   Low confidence and a degradation message.
7. Existing unit tests in planner.rs still pass (they don't use
   method_dependencies, so they exercise the fallback path).
8. `cargo fmt --all --check`, `cargo clippy -p aether-health -- -D warnings`,
   `cargo test -p aether-health` pass.

## Estimated Effort

1 Codex run. The agglomerative algorithm is ~80 lines. The dep filtering
is ~30 lines. The fused similarity is ~15 lines. Most of the existing
code stays unchanged.
