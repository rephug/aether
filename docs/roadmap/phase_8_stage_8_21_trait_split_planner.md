# Phase 8.21: Trait Split Planner

## Purpose

Extend AETHER's health planner to suggest trait decompositions, not just
file splits. The existing `suggest_split` in `planner.rs` uses community
detection to group symbols within a file into suggested modules. This
stage adds `suggest_trait_split` which uses the consumer × method usage
matrix (from 8.19's `aether_usage_matrix`) to group methods within a
trait into suggested sub-traits based on actual call patterns.

This is the difference between "these methods have similar names" (what
you'd get from reading source) and "these methods are always called
together by the same consumers" (what you get from graph data).

## Prerequisites

- Phase 8.19 merged (usage_matrix tool provides the consumer bitvector data)
- Phase 8.20 merged (method_dependencies in SIR enriches the grouping signal)

## What Changes

### 1. New function: `suggest_trait_split`

In `crates/aether-health/src/planner.rs`, add:

```rust
pub fn suggest_trait_split(
    trait_name: &str,
    trait_file: &str,
    methods: &[TraitMethod],
    consumer_matrix: &[ConsumerMethodUsage],
    method_dependencies: Option<&HashMap<String, Vec<String>>>,
) -> Option<TraitSplitSuggestion>
```

#### Input types

```rust
pub struct TraitMethod {
    pub name: String,
    pub qualified_name: String,
    pub symbol_id: String,
}

pub struct ConsumerMethodUsage {
    pub consumer_file: String,
    pub methods_used: Vec<String>,
}
```

These map directly to the `aether_usage_matrix` response. The MCP tool
or CLI gathers the data; the planner does the clustering.

#### Algorithm

```
1. Build consumer bitvectors:
   - Assign each method an index 0..N
   - For each method, create a BitVec where bit[i] = 1 if consumer[i]
     calls this method
   - Methods with identical bitvectors go in the same cluster

2. Relax strict identity to 80% overlap:
   - For methods not yet clustered, compute Jaccard similarity between
     their consumer bitvectors and each existing cluster's union bitvector
   - If Jaccard >= 0.80, merge into that cluster
   - If no cluster matches, create a new singleton cluster

3. Name each cluster:
   - If method_dependencies is available: find the most common dependency
     type across the cluster's methods — use that as the cluster name
     (e.g., "sir_state" if most methods depend on SirMetaRecord)
   - Otherwise: use the longest common prefix of method names, falling
     back to ranked token extraction (same STOPWORDS list as file planner)

4. Score each cluster:
   - consumer_isolation = fraction of the cluster's consumers that ONLY
     use methods in this cluster (not methods in other clusters)
   - Higher isolation = cleaner trait boundary
   - Flag clusters with consumer_isolation < 0.3 as "messy" — consumers
     would need multiple trait bounds

5. Flag cross-cutting methods:
   - Methods whose consumer bitvector overlaps significantly with 2+
     clusters are "cross-cutting" and should be noted as difficult to place

6. Produce TraitSplitSuggestion with per-cluster details
```

#### Output types

```rust
pub struct TraitSplitSuggestion {
    pub trait_name: String,
    pub trait_file: String,
    pub method_count: usize,
    pub suggested_traits: Vec<SuggestedSubTrait>,
    pub cross_cutting_methods: Vec<CrossCuttingMethod>,
    pub confidence: SplitConfidence,
}

pub struct SuggestedSubTrait {
    pub name: String,
    pub methods: Vec<String>,
    pub consumer_files: Vec<String>,
    pub consumer_isolation: f32,
    pub dominant_dependencies: Vec<String>,
    pub reason: String,
}

pub struct CrossCuttingMethod {
    pub method: String,
    pub overlapping_clusters: Vec<String>,
    pub reason: String,
}
```

### 2. New MCP tool: `aether_suggest_trait_split`

In `crates/aether-mcp/src/tools/health.rs` (or a new `trait_split.rs`):

#### Request

```json
{
  "trait_name": "Store",
  "file": "crates/aether-store/src/lib.rs"
}
```

#### Logic

1. Call `aether_usage_matrix` logic internally to get the consumer matrix
2. Look up the trait's SIR for `method_dependencies` (if available from 8.20)
3. Call `suggest_trait_split` with the gathered data
4. Return the `TraitSplitSuggestion`

#### Response

```json
{
  "schema_version": "1.0",
  "trait_name": "Store",
  "trait_file": "crates/aether-store/src/lib.rs",
  "method_count": 52,
  "suggested_traits": [
    {
      "name": "SirStateStore",
      "methods": ["write_sir_blob", "read_sir_blob", "upsert_sir_meta", "get_sir_meta"],
      "consumer_files": ["crates/aetherd/src/fsck.rs", "crates/aether-dashboard/src/api/anatomy.rs"],
      "consumer_isolation": 0.85,
      "dominant_dependencies": ["SirMetaRecord"],
      "reason": "Co-consumed by 7 files, 85% isolation — consumers rarely need other method groups"
    }
  ],
  "cross_cutting_methods": [
    {
      "method": "mark_removed",
      "overlapping_clusters": ["SymbolCatalogStore", "SirStateStore", "SemanticIndexStore"],
      "reason": "Called by consumers of 3 different clusters due to cascading cleanup"
    }
  ],
  "confidence": "high"
}
```

### 3. CLI integration

In `crates/aetherd/src/health_score.rs`, extend `--suggest-splits` to
also produce trait split suggestions for any trait flagged with
`trait_method_max` exceeding the threshold.

The existing `--suggest-splits` iterates over crate reports and calls
`suggest_split` for god files. Add a second pass that finds traits
exceeding `trait_method_max` and calls `suggest_trait_split`.

Output format (appended to existing suggest-splits output):

```
Trait split suggestion: Store (crates/aether-store/src/lib.rs)
  52 methods → 11 suggested sub-traits

  SirStateStore (4 methods, 85% isolation)
    write_sir_blob, read_sir_blob, upsert_sir_meta, get_sir_meta
    Consumers: fsck.rs, anatomy.rs, catalog.rs, ask.rs, ...

  SymbolCatalogStore (6 methods, 72% isolation)
    upsert_symbol, mark_removed, list_symbols_for_file, ...

  Cross-cutting: mark_removed (spans 3 clusters)
```

## Files to Modify

| File | Change |
|------|--------|
| `crates/aether-health/src/planner.rs` | Add `suggest_trait_split`, input/output types, bitvector clustering |
| `crates/aether-mcp/src/tools/health.rs` | Add `aether_suggest_trait_split` tool (or new `trait_split.rs`) |
| `crates/aether-mcp/src/tools/router.rs` | Register `aether_suggest_trait_split` |
| `crates/aether-mcp/src/tools/mod.rs` | Add module if using separate file |
| `crates/aetherd/src/health_score.rs` | Extend `--suggest-splits` for trait suggestions |

## Pass Criteria

1. `suggest_trait_split` clusters methods by consumer bitvector overlap.
2. Clusters with identical consumer sets are grouped first, then 80%+ Jaccard.
3. Each cluster has a derived name, consumer list, and isolation score.
4. Cross-cutting methods (overlapping 2+ clusters) are flagged separately.
5. `aether_suggest_trait_split` MCP tool returns structured suggestion.
6. `--suggest-splits` CLI output includes trait suggestions alongside file suggestions.
7. Running against Store produces clusters that roughly match the 11-subtrait decomposition we derived manually (validation that the algorithm works).
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, per-crate tests pass.

## Estimated Effort

1–2 Codex runs. The bitvector clustering is ~100 lines. The MCP tool
reuses usage_matrix logic internally. The CLI integration is formatting.
