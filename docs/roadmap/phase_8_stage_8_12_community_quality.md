# Phase 8 — Stage 8.12: Community Detection Quality

## Purpose

Improve the split planner's community detection so it produces
actionable module-split suggestions instead of hundreds of
micro-communities. The self-analysis of AETHER revealed 434+
communities for a single 5869-line file, with module names like
`from_ops_162` and `normalize_ops_23`. This stage fixes the
planner's clustering pipeline to produce 5-10 meaningful groups
per hot file.

## Scope

This stage improves **planner-local community detection only**.
It does NOT change the global community snapshot used by drift
detection and boundary violation analysis. The existing
`mine-coupling → global Louvain → community_snapshot` pipeline
is untouched.

Specifically, after this stage:
- `health-score --suggest-splits` produces better split suggestions
- `communities` CLI output is unchanged
- `drift-report` boundary violations are unchanged
- "Boundary Leaker" archetype counts are unchanged (fixing those
  requires global snapshot improvements — see Future Work)

## What this stage does NOT change

- Global community snapshot (stored in `community_snapshot` table)
- Health scoring formulas (`combined_score`, `compute_crate_penalty`)
- Existing CLI commands, API endpoints, MCP tools, LSP hover
- Store trait or any Store implementations
- Coupling mining (git co-change pair extraction)
- Drift detection or boundary violation analysis
- Dashboard panels
- Edge extraction in `aether-parse`

## Root causes addressed

### 1. Global scope → micro-communities
Louvain on all 3361 symbols with sparse edges shatters into hundreds
of tiny 2-3 symbol groups. File-scoped detection on 50-200 symbols
with enriched edges produces 5-10 groups.

### 2. Isolated passive symbols → orphans
Struct/enum/trait definitions have zero outgoing CALLS edges. They
appear as disconnected nodes. Type-anchor rescue, container rescue,
and selective semantic rescue progressively connect them.

### 3. Test function pollution
Test functions inflate community count and produce meaningless
suggestions. They must be filtered BEFORE graph construction.

### 4. No resolution control
Standard Louvain has no granularity knob. A resolution parameter
and a post-clustering merge pass provide two levels of control.

### 5. Poor module naming
First-token prefix produces `default_ops` for half of
`aether-config`. Token-frequency naming with stopwords fixes this.

### 6. No observability or confidence signal
Previous planner had no instrumentation. This stage adds diagnostics,
data-driven confidence scoring, and a stability check.

## Implementation

### 1. New module: planner community detection

**File: `crates/aether-health/src/planner_communities.rs`**

This module owns the planner's file-scoped clustering pipeline.
It does NOT modify or depend on the global coupling/community
code in `aether-analysis`. It consumes edges and embeddings as
inputs, not by reaching into stores directly.

Public API:

```rust
pub struct FileCommunityConfig {
    pub semantic_rescue_threshold: f32,  // default 0.70
    pub semantic_rescue_max_k: usize,    // default 3
    pub louvain_resolution: f64,         // default 0.5
    pub min_community_size: usize,       // default 3
}

pub struct FileSymbol {
    pub symbol_id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind, // Struct, Enum, Trait, TypeAlias, Function, Method, Const, etc.
    pub is_test: bool,
    pub embedding: Option<Vec<f32>>,
}

pub struct PlannerDiagnostics {
    pub symbols_total: usize,
    pub symbols_filtered_test: usize,
    pub symbols_anchored_type: usize,
    pub symbols_rescued_container: usize,
    pub symbols_rescued_semantic: usize,
    pub symbols_loner: usize,
    pub communities_before_merge: usize,
    pub communities_after_merge: usize,
    pub embedding_coverage_pct: f32,
    pub confidence: f32,               // 0.0..=1.0
    pub confidence_label: String,      // "high", "medium", "low"
    pub stability_score: f32,          // 0.0..=1.0, from perturbation test
}

pub fn detect_file_communities(
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
) -> (Vec<(String, usize)>, PlannerDiagnostics)
```

Algorithm (in order):

**Step 1 — Filter tests.**
Remove all symbols where `is_test == true` from the working set.
Remove all edges where source or target is a test symbol.

Test detection: prefer explicit metadata when available (e.g.,
`symbol_kind == Test` or a test flag from the store). Fall back
to heuristics only when metadata is absent:
- name starts with `test_` or matches `*_test`, `*_tests`, `*_spec`
- file path contains `/tests/` or `/test/`

This happens BEFORE any graph construction. Record count in
diagnostics.

**Step 2 — Type-anchor rescue (hard constraint).**
For each symbol whose `kind` is Struct, Enum, Trait, or TypeAlias
with qualified name `X`:
  a. Find all other symbols in the file whose qualified-name stem
     (everything before the last `::`) equals `X`.
  b. Add strong synthetic edges between the type definition and
     every matching method/function (both directions).

This is a **hard pre-clustering rule**, not a heuristic. A type
definition and its impl methods MUST always land in the same
community. This runs before degree checks and before any
heuristic rescue.

Record count of symbols connected by type-anchor in diagnostics.

**Step 3 — Container/locality rescue.**
For each symbol with degree == 0 in the structural graph
(after type-anchor edges):
  a. Extract the qualified-name stem.
  b. Find all other symbols in the file with the same stem
     that are NOT already connected by type-anchor.
  c. Add edges between them.

This is cheaper and more trustworthy than semantic rescue. It
catches "methods on the same type" that weren't caught by
type-anchor (e.g., free functions with a shared prefix).
Record rescued count in diagnostics.

**Step 4 — Selective semantic rescue.**
For each symbol STILL with degree == 0 (isolated after type-anchor
and container rescue) or degree == 1 (weakly connected):
  a. Compute cosine similarity against all other symbols in the file
     that have embeddings.
  b. If the best similarity >= `semantic_rescue_threshold` (default
     0.70), add synthetic edges to the top-k most similar symbols
     (default k=3), with `edge_kind = "semantic"`.
  c. Skip symbols without embeddings.

This is NOT all-pairs. Only isolated/low-degree symbols get semantic
rescue. High-degree symbols already have structural context. This
avoids the "mushy blob" problem where 100 similar `default_*`
functions all merge into one group. Record rescued count in
diagnostics.

**Step 5 — Connected components.**
Compute connected components on the enriched graph. Components with
only 1 symbol are tagged as "loners." Loners are excluded from
Louvain AND from the final community assignments. They will NOT
appear in split suggestions. If a file has only loners after
filtering, `suggest_split()` returns `None`. Record loner count
in diagnostics.

**Step 6 — Louvain per component.**
For each connected component with >= 2 symbols, run
`louvain_with_resolution_sync(component_edges, resolution)`.
Community IDs are remapped to be globally unique across components.
Record community count in diagnostics (before merge).

**Step 7 — Merge small communities (component-bounded).**
After Louvain, any community with fewer than `min_community_size`
symbols (default 3) is merged into the nearest larger community
within the SAME connected component.

"Nearest" = the community that shares the most structural edges
with the small community's symbols.

**Fallback when no structural winner exists within the component:**
use the community with strongest semantic centroid affinity (mean
cosine similarity of community embeddings). If no embeddings are
available for either community, leave the small community unmerged
and flag it in diagnostics (this lowers confidence).

**Merges CANNOT cross disconnected components** unless semantic
rescue (Step 4) already created a bridge edge between them.

Record community count in diagnostics (after merge).

**Step 8 — Stability check.**
Run the pipeline a second time with perturbed parameters:
- semantic_rescue_threshold + 0.05
- louvain_resolution + 0.1

Compare the two partition results using pairwise co-membership
Jaccard similarity: for each pair of symbols, check if they are
in the same community in both runs. Stability score = Jaccard
index (0.0 = completely different, 1.0 = identical).

Do NOT compare raw community IDs (they are arbitrary after
remapping). This costs one extra Louvain run per component
(milliseconds).

**Step 9 — Compute confidence.**
Start from 1.0 and subtract:
- `rescue_ratio = (rescued_container + rescued_semantic) /
  (total - filtered_test)` → subtract `0.3 * rescue_ratio`
- `loner_ratio = loners / (total - filtered_test)` →
  subtract `0.2 * loner_ratio`
- `embedding_gap = 1.0 - embedding_coverage_pct` →
  subtract `0.2 * embedding_gap`
- `instability = 1.0 - stability_score` →
  subtract `0.3 * instability`

Clamp to 0.0..=1.0. Map:
- >= 0.7: "high"
- >= 0.4: "medium"
- < 0.4: "low"

**Step 10 — Return assignments + diagnostics.**
Return `Vec<(symbol_id, community_id)>` for all non-test,
non-loner symbols, plus `PlannerDiagnostics` with all counts
and computed confidence.

### 2. Resolution parameter for Louvain

**File: `crates/aether-graph-algo/src/lib.rs`**

Add alongside existing `louvain_sync`:

```rust
pub fn louvain_with_resolution_sync(
    edges: &[GraphAlgorithmEdge],
    resolution: f64,
) -> Vec<(String, usize)>
```

The resolution parameter γ modifies the modularity gain formula.
When γ < 1.0, the algorithm favors larger communities. When γ = 1.0,
it reduces to standard Louvain. When γ > 1.0, it favors smaller ones.

The existing `louvain_sync` remains unchanged — it calls the new
function with resolution = 1.0 for backward compatibility.

### 3. Improved planner naming

**File: `crates/aether-health/src/planner.rs`**

Replace the current first-token prefix heuristic:

**Step 1 — Tokenize.**
Split each symbol name by `_` delimiter.

**Step 2 — Remove stopwords.**
Filter out tokens: `default`, `new`, `from`, `into`, `load`, `save`,
`get`, `set`, `is`, `has`, `with`, `for`, `the`, `and`, `fn`, `test`,
`mock`, `impl`, `try`, `run`, `do`.

**Step 3 — Frequency rank.**
Compute token frequency across the community's symbol names.
Primary module name = most frequent non-stopword token + `_ops`.

**Step 4 — Disambiguation.**
If two communities produce the same module name, append the
second-most-frequent token: `store_migration_ops` vs
`store_query_ops`. If still colliding, append a third token.
Continue until unique.

Do NOT merge communities by label collision. Two communities
landing on the same name is evidence that naming is lossy, NOT
that they belong together. Always disambiguate, never merge.

**Step 5 — Alias normalization.**
Normalize inflection variants to a canonical form before
frequency ranking:
`note` / `notes` → `note`,
`migration` / `migrate` → `migration`,
`test` / `tests` → `test`,
`store` / `stores` → `store`.
Use a small hardcoded alias table — not a stemming library.
This prevents two communities from getting different names
that are really the same concept.

**Step 6 — File path generation (output layer only).**
For each module name, generate `suggested_file_path`:
- Strip `_ops` suffix
- Pluralize where natural (e.g., `migration` → `migrations`)
- Prepend the crate's `src/` directory
- Example: `migration_ops` → `crates/aether-store/src/migrations.rs`

Both `module_name` and `suggested_file_path` are included in
the output. Path generation DOES NOT influence clustering. It
is a pure presentation-layer mapping.

### 4. Updated planner flow

**File: `crates/aether-health/src/planner.rs`**

Update `suggest_split()` to use the new pipeline:

```rust
pub fn suggest_split(
    file_path: &str,
    crate_score: u32,
    structural_edges: &[GraphAlgorithmEdge],
    symbols: &[FileSymbol],
    config: &FileCommunityConfig,
) -> Option<(SplitSuggestion, PlannerDiagnostics)>
```

The signature changes from taking `PlannerCommunityAssignment`
and `PlannerSymbolRecord` to taking `GraphAlgorithmEdge` and
`FileSymbol`. The planner now owns community detection internally
instead of consuming pre-computed global assignments.

The old types (`PlannerCommunityAssignment`, `PlannerSymbolRecord`)
are removed. Callers (CLI, MCP, dashboard) must adapt to provide
edges and symbols instead.

`SuggestedModule` gains a new field:
```rust
pub suggested_file_path: String,
```

Loners are excluded from `SplitSuggestion`. If the only non-test
symbols are loners, return `None`.

### 5. Caller updates

**File: `crates/aetherd/src/health_score.rs`**

The CLI health-score handler must now:
1. For each hot file (score >= 50 with `max_file_path`):
   a. Load symbol records for the file from SqliteStore.
   b. Load dependency edges between those symbols from GraphStore.
   c. Load embeddings for those symbols via VectorStore trait
      (use existing API — do NOT add a new store helper).
   d. Mark test symbols: prefer explicit test metadata from store
      when available, fall back to name/path heuristics when absent.
   e. Populate `qualified_name` and `kind` for type-anchor and
      container rescue.
   f. Call `suggest_split()` with the raw data.
   g. Print `PlannerDiagnostics` below the split suggestion in
      table mode. Include confidence label and stability score.
      Include full diagnostics in JSON output.

**File: `crates/aether-mcp/src/lib.rs`**

The `aether_health_explain` tool's split suggestion block must
adapt to the new `suggest_split()` signature. Include diagnostics
and confidence in the tool output text.

### 6. Config

**Add new section `[planner]` in `aether-config`:**

```toml
[planner]
semantic_rescue_threshold = 0.70   # min cosine sim for rescue edge
semantic_rescue_max_k = 3          # max semantic neighbors per symbol
community_resolution = 0.5        # Louvain γ (< 1.0 = larger groups)
min_community_size = 3             # merge communities smaller than this
```

Do NOT put these under `[coupling]`. These are planner settings,
conceptually separate from coupling mining.

Normalize and clamp:
- `semantic_rescue_threshold`: 0.3..=0.95, default 0.70
- `semantic_rescue_max_k`: 1..=10, default 3
- `community_resolution`: 0.1..=3.0, default 0.5
- `min_community_size`: 1..=20, default 3

## Tests

### aether-graph-algo
- `louvain_with_resolution_produces_fewer_communities_at_low_gamma`
  Graph with 3 natural clusters + weak bridges. γ=1.0: 3 communities.
  γ=0.3: 1-2 communities.
- `louvain_with_resolution_one_matches_standard_louvain`
  Same edges → same assignments as `louvain_sync`.
- `louvain_with_resolution_high_gamma_more_communities`
  γ=2.0 → more communities than γ=1.0.

### aether-health (planner_communities)
- `filter_tests_before_graph_construction`
  Pass 10 symbols (3 test). Resulting graph has 7 nodes. Test
  symbols do not appear in any community assignment.
- `type_anchor_connects_definition_to_methods`
  Struct definition `Foo` and methods `Foo::bar`, `Foo::baz`
  → all in same community. Struct has degree > 0 after anchor.
- `type_anchor_does_not_cross_types`
  `Foo::bar` and `Bar::baz` → NOT connected by type-anchor.
- `container_rescue_connects_same_stem_after_anchor`
  Two free functions sharing a qualified-name stem, not caught
  by type-anchor → connected by container rescue.
- `semantic_rescue_connects_isolated_symbols`
  One isolated symbol (degree 0 after type-anchor and container
  rescue) with high cosine similarity → gets assigned to community.
- `semantic_rescue_skips_high_degree_symbols`
  A symbol with degree >= 2 does not get semantic rescue edges.
- `semantic_rescue_respects_top_k`
  Isolated symbol similar to 5 others at k=3 → only 3 edges added.
- `loners_excluded_from_output`
  Symbols that remain degree-0 after all rescue steps do not appear
  in the community assignment output.
- `all_loners_returns_none`
  File where every non-test symbol is a loner → `suggest_split()`
  returns `None`.
- `merge_pass_absorbs_small_communities`
  After Louvain, a community with 1 symbol gets merged into nearest
  community within the same component.
- `merge_pass_respects_component_boundaries`
  Small community in component A is NOT merged into a community
  in component B.
- `merge_fallback_uses_semantic_when_no_structural_winner`
  Small community with no structural edges to any larger community
  within its component → merges by semantic centroid affinity.
- `merge_fallback_leaves_unmerged_when_no_signal`
  Small community with no structural or semantic signal → left
  unmerged, confidence lowered.
- `connected_components_isolate_louvain_runs`
  Graph with 2 disconnected subgraphs → Louvain runs separately,
  community IDs unique.
- `stability_check_returns_high_for_stable_graph`
  Well-separated clusters → stability score > 0.9.
- `stability_check_detects_unstable_partition`
  Weakly separated clusters → stability score < 0.5 when
  threshold is perturbed.
- `confidence_reflects_diagnostics`
  High rescue ratio + low embedding coverage → confidence < 0.4
  ("low"). Low rescue + high coverage + stable → confidence > 0.7
  ("high").
- `diagnostics_reports_accurate_counts`
  Full pipeline: verify all diagnostic fields are correct.
- `full_pipeline_produces_actionable_groups`
  Integration test: 50 symbols, mix of structural edges and
  isolated symbols with embeddings → expect 4-8 communities,
  no loners in output, no test symbols, confidence > 0.

### aether-health (planner naming)
- `naming_uses_token_frequency_not_prefix`
  Community: `run_migrations`, `migration_v6_renames`,
  `migration_from_v2` → module name `migration_ops`.
- `naming_skips_stopwords`
  Community: `default_log_level`, `default_port`,
  `default_enabled` → module name NOT `default_ops`.
- `naming_disambiguates_collisions_without_merging`
  Two communities both produce `store_ops` → one becomes
  `store_migration_ops`, the other `store_query_ops`. They
  are NOT merged.
- `naming_normalizes_aliases`
  Tokens `note` and `notes` → normalized to `note` before
  frequency ranking.
- `naming_generates_file_paths`
  Module `migration_ops` → `crates/aether-store/src/migrations.rs`.
  Module `project_note_ops` → `crates/aether-store/src/project_notes.rs`.
- `planner_signature_change_compiles`
  Verify the new `suggest_split()` signature works with edges
  and FileSymbol inputs.

### aether-health (ablation harness — #[ignore])
- `ablation_aether_store`
- `ablation_aether_config`
- `ablation_aether_mcp`

These are `#[ignore]` integration tests that run against a live
`.aether/` directory. They are NOT run in CI. Each test runs the
pipeline on the named file with six configurations and prints a
comparison table:

  1. Baseline (structural edges only, no rescue, no merge, γ=1.0)
  2. + test filtering only
  3. + type-anchor rescue
  4. + container rescue + semantic rescue
  5. + lower γ (0.5)
  6. + merge pass (full pipeline)

Each row shows: community count, largest community size, smallest
community size, loner count, confidence, stability score, top 3
module names. This tells you which lever matters per file profile.

Run manually:
```bash
cargo test -p aether-health -- ablation --ignored --nocapture
```

### aether-config
- `planner_config_normalizes_new_fields`
  Missing fields get defaults. Out-of-range values clamped.
- `planner_config_section_parses`
  Valid `[planner]` TOML round-trips through parse/serialize.

## Validation

```bash
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

cargo fmt --check
cargo clippy -p aether-graph-algo -p aether-analysis -p aether-health \
  -p aether-config -p aether-mcp -p aether-dashboard -p aetherd -- -D warnings
cargo test -p aether-graph-algo
cargo test -p aether-health
cargo test -p aether-config
cargo test -p aether-mcp
cargo test -p aether-dashboard
cargo test -p aetherd
```

Manual verification after merge:

```bash
# Re-run health score with improved planner
aetherd health-score --workspace /home/rephu/projects/aether \
  --semantic --suggest-splits --output table

# Expected: 5-10 modules per God File, meaningful names,
# suggested file paths, no test functions, no loners,
# diagnostics + confidence printed below each suggestion

# Run ablation to validate thresholds
cargo test -p aether-health -- ablation --ignored --nocapture
```

## Decisions

- **#59**: File-scoped community detection is planner-local; global
  snapshot unchanged. Boundary Leaker fix deferred to 8.13.
- **#60**: Selective semantic rescue for isolated/low-degree symbols
  only (NOT all-pairs). Threshold default 0.70, top-k default 3.
- **#61**: Louvain resolution γ = 0.5 for file-scoped planner.
  Global Louvain unchanged at γ = 1.0 (implicit).
- **#62**: Test symbols filtered BEFORE graph construction. Prefer
  explicit test metadata, fall back to name/path heuristics.
- **#63**: Config under `[planner]` section, not `[coupling]`.
- **#64**: Naming uses token frequency with stopword list and
  alias normalization. Label collisions are ALWAYS disambiguated,
  NEVER merged.
- **#65**: Loners excluded from suggestions, not fake-placed.
- **#66**: Type-anchor rescue is a hard pre-clustering constraint
  for struct/enum/trait/type-alias definitions and their impl
  methods. Runs before container rescue and degree checks.
- **#67**: Container/locality rescue by qualified-name stem runs
  after type-anchor, before semantic rescue.
- **#68**: Post-Louvain merge is strictly component-bounded.
  Fallback: semantic centroid affinity when no structural winner;
  leave unmerged and lower confidence when no signal at all.
- **#69**: PlannerDiagnostics with full instrumentation including
  data-driven confidence (0.0-1.0 scale) from rescue ratio,
  loner ratio, embedding coverage, and stability score.
- **#70**: Stability check via co-membership Jaccard on perturbed
  parameters (threshold ±0.05, γ ±0.1). Costs one extra Louvain
  run per component.
- **#71**: Output includes `suggested_file_path` per module as
  pure presentation layer. Path generation does not influence
  clustering.

## Future work

**Stage 8.13 — Global community quality:**
- Improve the stored community snapshot used by drift/boundary
- Apply selective rescue to global Louvain (conservative settings)
- This fixes Boundary Leaker counts and orphan counts
- Separate stage: affects drift-report, needs backward-compat testing

**Stage 8.14 or 9.x — Enhanced edge extraction:**
- Add IMPLEMENTS, TYPE_REF, FIELD_ACCESS edge types to `aether-parse`
- Reduces orphans structurally (less rescue needed)
- High-value follow-up once 8.12 results are measured
- Larger scope: tree-sitter walker changes for Rust + TypeScript

## End-of-stage git sequence

```bash
cd /home/rephu/projects/aether-phase8-community-quality
git push origin feature/phase8-stage8-12-community-quality

# Create PR via GitHub web UI, then after merge:
cd /home/rephu/projects/aether
git switch main
git pull --ff-only
git worktree remove ../aether-phase8-community-quality
git branch -D feature/phase8-stage8-12-community-quality
git worktree prune
```
