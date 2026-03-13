# Codex Prompt — Phase 8.21: Trait Split Planner

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
- `docs/roadmap/phase_8_stage_8_21_trait_split_planner.md`
- `docs/hardening/phase8_stage8_21_session_context.md`

Then read these source files:
- `crates/aether-health/src/planner.rs` (existing file split planner — follow this pattern)
- `crates/aether-mcp/src/tools/usage_matrix.rs` (consumer matrix logic to reuse)
- `crates/aether-mcp/src/tools/health.rs` (health MCP tools — add new tool here or in new file)
- `crates/aether-mcp/src/tools/router.rs` (tool registration)
- `crates/aetherd/src/health_score.rs` (--suggest-splits CLI)
- `crates/aether-sir/src/lib.rs` (SirAnnotation with method_dependencies)
- `crates/aether-store/src/symbols.rs` (symbol queries)
- `crates/aether-store/src/graph.rs` (edge queries — store_get_callers)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -b feature/phase8-stage8-21-trait-split-planner /home/rephu/aether-phase8-trait-planner
cd /home/rephu/aether-phase8-trait-planner
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any is false, STOP and report:

1. `suggest_split` in `planner.rs` takes `structural_edges`, `symbols`,
   and `config` as inputs and returns `Option<(SplitSuggestion, PlannerDiagnostics)>`.
   It does NOT access Store directly — it's a pure function.

2. The usage_matrix tool in `usage_matrix.rs` queries `store.list_symbols_for_file`
   and `store.get_callers` to build the consumer matrix. Identify the exact
   functions and their signatures.

3. `SirAnnotation` has `method_dependencies: Option<HashMap<String, Vec<String>>>`.
   If this field is missing (8.20 not merged yet), the planner must work
   without it — method_dependencies is an enrichment signal, not required.

4. `health_score.rs` has a section that collects split suggestions when
   `args.suggest_splits` is true. Find the exact function name and how it
   iterates over crate reports.

5. The `STOPWORDS` list and token-ranking logic in `planner.rs` are reusable.
   Check if they're already `pub` or need to be exported.

6. `aether-health` does NOT depend on `aether-mcp`. The planner must stay
   in `aether-health` without importing MCP types.

## CHANGE 1: Trait split planner in aether-health

Create the planner function in `crates/aether-health/src/planner.rs`
(add to the existing file, after `suggest_split`).

### Input types (add at top of file)

```rust
#[derive(Debug, Clone)]
pub struct TraitMethod {
    pub name: String,
    pub qualified_name: String,
    pub symbol_id: String,
}

#[derive(Debug, Clone)]
pub struct ConsumerMethodUsage {
    pub consumer_file: String,
    pub methods_used: Vec<String>,
}
```

### Output types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitSplitSuggestion {
    pub trait_name: String,
    pub trait_file: String,
    pub method_count: usize,
    pub suggested_traits: Vec<SuggestedSubTrait>,
    pub cross_cutting_methods: Vec<CrossCuttingMethod>,
    pub confidence: SplitConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedSubTrait {
    pub name: String,
    pub methods: Vec<String>,
    pub consumer_files: Vec<String>,
    pub consumer_isolation: f32,
    pub dominant_dependencies: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossCuttingMethod {
    pub method: String,
    pub overlapping_clusters: Vec<String>,
    pub reason: String,
}
```

`SplitConfidence` already exists in the file — reuse it.

### Function signature

```rust
pub fn suggest_trait_split(
    trait_name: &str,
    trait_file: &str,
    methods: &[TraitMethod],
    consumer_matrix: &[ConsumerMethodUsage],
    method_dependencies: Option<&HashMap<String, Vec<String>>>,
) -> Option<TraitSplitSuggestion>
```

### Algorithm

```rust
// Step 1: Assign method indices
let method_names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
let method_index: HashMap<&str, usize> = method_names.iter().enumerate()
    .map(|(i, name)| (*name, i))
    .collect();
let n_methods = methods.len();
let n_consumers = consumer_matrix.len();

// Step 2: Build consumer bitvectors per method
// bitvec[method_idx] = set of consumer indices that call this method
let mut method_consumers: Vec<HashSet<usize>> = vec![HashSet::new(); n_methods];
for (consumer_idx, usage) in consumer_matrix.iter().enumerate() {
    for method_name in &usage.methods_used {
        if let Some(&method_idx) = method_index.get(method_name.as_str()) {
            method_consumers[method_idx].insert(consumer_idx);
        }
    }
}

// Step 3: Cluster methods with identical consumer sets
let mut clusters: Vec<Vec<usize>> = Vec::new(); // Vec of method indices
let mut cluster_consumers: Vec<HashSet<usize>> = Vec::new();
let mut assigned: Vec<bool> = vec![false; n_methods];

for i in 0..n_methods {
    if assigned[i] { continue; }
    let mut cluster = vec![i];
    assigned[i] = true;
    for j in (i+1)..n_methods {
        if assigned[j] { continue; }
        if method_consumers[i] == method_consumers[j] {
            cluster.push(j);
            assigned[j] = true;
        }
    }
    cluster_consumers.push(method_consumers[i].clone());
    clusters.push(cluster);
}

// Step 4: Absorb unassigned methods with >=80% Jaccard overlap
// (All methods should be assigned after step 3, but check for
// methods with empty consumer sets — they're "uncalled")

// Step 5: Name each cluster
// Use method_dependencies to find dominant types if available.
// Otherwise use ranked token extraction from method names.

// Step 6: Compute consumer_isolation per cluster
// isolation = fraction of the cluster's consumers that ONLY call
// methods within this cluster

// Step 7: Find cross-cutting methods
// Methods whose consumer set overlaps with 2+ clusters significantly
```

Implement this fully. Use the STOPWORDS list and token-ranking from
the existing `suggest_split` for naming. The bitvector operations are
simple set operations — use `HashSet` for clarity, not actual bit
manipulation.

Return `None` if methods.len() < 2 or consumer_matrix is empty.

Set confidence based on:
- High: all clusters have isolation >= 0.6 and no cross-cutting methods
- Medium: some clusters have isolation < 0.6 or 1-2 cross-cutting methods
- Low: many clusters have isolation < 0.3 or 3+ cross-cutting methods

## CHANGE 2: MCP tool

Add `aether_suggest_trait_split` to `crates/aether-mcp/src/tools/health.rs`
(or create `crates/aether-mcp/src/tools/trait_split.rs`).

### Request

```rust
pub struct AetherSuggestTraitSplitRequest {
    pub trait_name: String,
    pub file: Option<String>,
}
```

### Logic

1. Resolve the trait symbol (same as usage_matrix: search symbols by name,
   filter by file and kind = "trait")
2. Find child methods (same qualified_name prefix query as usage_matrix)
3. Build consumer matrix (same edge queries as usage_matrix)
4. Look up the trait's SIR for method_dependencies (optional — if SIR
   exists and has the field, pass it; if not, pass None)
5. Call `suggest_trait_split` from aether-health
6. Return the result as JSON

### Registration

```rust
#[tool(
    name = "aether_suggest_trait_split",
    description = "Suggest how to decompose a large trait into smaller sub-traits based on consumer usage patterns"
)]
```

## CHANGE 3: CLI integration

In `crates/aetherd/src/health_score.rs`, in the `--suggest-splits` path:

After collecting file split suggestions, add a second pass:

1. For each crate report, check if any diagnostic flags `trait_method_max`
   exceeding threshold
2. If so, find the offending trait(s) by querying symbols in that crate
   with kind = "trait" and counting their child methods
3. Build the consumer matrix from Store edge queries
4. Call `suggest_trait_split`
5. Format and append to the output

The trait suggestion output goes AFTER the file suggestions:

```
Trait split suggestions:
  Store (crates/aether-store/src/lib.rs) — 52 methods → 11 clusters
    SirStateStore (4 methods, 85% isolation): write_sir_blob, read_sir_blob, ...
    SymbolCatalogStore (6 methods, 72% isolation): upsert_symbol, ...
    Cross-cutting: mark_removed (spans 3 clusters)
```

## TESTS

In `crates/aether-health/src/planner.rs` tests:

1. **Basic clustering:** 4 methods, 2 consumers. Consumer A calls methods
   1+2, consumer B calls methods 3+4. Verify 2 clusters formed.

2. **Overlapping consumers:** 4 methods, 2 consumers. Consumer A calls
   methods 1+2+3, consumer B calls methods 2+3+4. Method 2+3 have
   identical consumer sets and cluster together. Verify 3 clusters:
   {1}, {2,3}, {4}.

3. **Cross-cutting detection:** 3 methods, 3 consumers. Method 1 called
   by all 3 consumers. Methods 2+3 each called by 1 consumer only.
   Verify method 1 flagged as cross-cutting.

4. **Empty consumers:** Methods with zero callers appear in uncalled list.

5. **method_dependencies naming:** Provide method_dependencies mapping.
   Verify cluster names derive from dominant dependency types.

In `crates/aether-mcp/tests/mcp_tools.rs`:

6. **MCP tool integration:** Create a workspace with a trait having 4+
   methods and 2+ consumer files. Call `aether_suggest_trait_split`.
   Verify response has clusters.

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy -p aether-health -- -D warnings
cargo test -p aether-health
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

## COMMIT

```bash
git add -A
git commit -m "Add trait split planner with consumer-bitvector clustering and aether_suggest_trait_split MCP tool"
```

Do NOT push automatically. Report commit SHA and wait for review.

Push command (after review):
```bash
git push -u origin feature/phase8-stage8-21-trait-split-planner
```
