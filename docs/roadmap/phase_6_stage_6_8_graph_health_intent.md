# Phase 6 — The Chronicler

## Stage 6.8 — Graph Health Metrics + Intent Verification

### Purpose
Two capabilities in one stage because both are primarily *queries on existing data* rather than new data pipelines.

**Graph Health Metrics:** Apply CozoDB's built-in graph algorithms to the dependency graph and combine results with SIR quality data to produce an actionable codebase health dashboard. No existing tool does this at the semantic level — SonarQube counts lines and cyclomatic complexity; AETHER can say "this function is the most critical in your codebase (PageRank 0.94), changed meaning 3 times this month (semantic drift), has no test guards, and sits at a module boundary violation."

**Intent Verification:** Before/after refactor comparison of SIR to detect unintended semantic changes — even when all tests pass. Tests verify *behavior*; AETHER verifies *intent preservation*.

### Part A: Graph Health Metrics

#### Algorithm

Run CozoDB built-in graph algorithms on the `dependency_edges` relation, then enrich with SIR and access data.

```
1. PageRank — Most critical symbols:
   ?[symbol, rank] := pagerank(*dependency_edges[], symbol, rank)
   → Symbols everything depends on. Highest blast radius on failure.

2. Community Detection (Louvain) — Actual module boundaries:
   ?[symbol, community] := community_detection_louvain(*dependency_edges[], symbol, community)
   → Compare communities vs. directory structure. Symbols logically together but physically scattered.

3. Betweenness Centrality — Bottleneck symbols:
   ?[symbol, centrality] := betweenness_centrality(*dependency_edges[], symbol, centrality)
   → If these break, most paths through the codebase are disrupted.

4. Cycle Detection (SCC) — Circular dependencies:
   ?[symbol, component] := strongly_connected_components(*dependency_edges[], symbol, component)
   → Components with >1 member are circular dependency clusters.

5. Connected Components — Orphaned code:
   ?[symbol, component] := connected_components(*dependency_edges[], symbol, component)
   → Components not connected to main application entry points = dead code candidates.
```

#### Enrichment: Cross-Layer Risk Score

For each symbol, combine graph metrics with SIR quality and access data:

```
risk_score = weighted combination of:
  - pagerank (high = more critical, more risk if it fails)
  - has_sir (false = undocumented critical code)
  - test_coverage (from 6.3 tested_by edges: 0 tests = higher risk)
  - drift_magnitude (from 6.6: drifting symbols are riskier)
  - access_recency (recently accessed = actively used, higher impact)

Composite:
  risk = 0.3 * pagerank_normalized
       + 0.25 * (1.0 - test_coverage_ratio)
       + 0.2 * drift_magnitude
       + 0.15 * (1.0 if no_sir else 0.0)
       + 0.1 * access_recency_factor
```

A symbol with high PageRank, no tests, recent semantic drift, and no SIR = **ticking time bomb.**

#### MCP Tool: `aether_health`

**Request:**
```json
{
  "include": ["critical_symbols", "bottlenecks", "cycles", "orphans", "risk_hotspots"],
  "limit": 10,
  "min_risk": 0.5
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "analysis": {
    "total_symbols": 342,
    "total_edges": 1847,
    "communities_detected": 8,
    "cycles_detected": 2,
    "orphaned_subgraphs": 3,
    "analyzed_at": 1708000000000
  },
  "critical_symbols": [
    {
      "symbol_id": "abc123",
      "symbol_name": "AppContext",
      "file": "src/context.rs",
      "pagerank": 0.94,
      "betweenness": 0.82,
      "dependents_count": 47,
      "has_sir": true,
      "test_count": 2,
      "drift_magnitude": 0.0,
      "risk_score": 0.71,
      "risk_factors": ["high pagerank", "low test coverage relative to criticality"]
    }
  ],
  "bottlenecks": [
    {
      "symbol_id": "def456",
      "symbol_name": "database_pool",
      "file": "src/db/pool.rs",
      "betweenness": 0.91,
      "pagerank": 0.67,
      "note": "91% of dependency paths pass through this symbol"
    }
  ],
  "cycles": [
    {
      "cycle_id": 1,
      "symbols": [
        {"id": "g1", "name": "parse_config", "file": "src/config/parser.rs"},
        {"id": "g2", "name": "validate_config", "file": "src/config/validator.rs"},
        {"id": "g3", "name": "resolve_defaults", "file": "src/config/defaults.rs"}
      ],
      "edge_count": 3,
      "note": "Circular: parse_config → validate_config → resolve_defaults → parse_config"
    }
  ],
  "orphans": [
    {
      "subgraph_id": 1,
      "symbols": [
        {"id": "h1", "name": "old_parser", "file": "src/legacy/parser.rs"},
        {"id": "h2", "name": "legacy_format", "file": "src/legacy/format.rs"}
      ],
      "note": "No inbound dependencies from main application — dead code candidate"
    }
  ],
  "risk_hotspots": [
    {
      "symbol_id": "jkl012",
      "symbol_name": "process_payment",
      "file": "src/payments/processor.rs",
      "risk_score": 0.88,
      "risk_factors": [
        "pagerank 0.78 (top 5%)",
        "semantic drift 0.31 over last 50 commits",
        "only 2 test guards for 7 edge cases in SIR",
        "boundary violation: calls into 2 other communities"
      ]
    }
  ]
}
```

### Part B: Intent Verification

#### Purpose
After a refactor, compare pre-refactor SIR snapshots against post-refactor SIR to detect unintended semantic changes — even when all tests pass.

#### Workflow
1. **Before refactor:** Agent (or human) calls `aether_snapshot_intent` to capture current SIR state for affected symbols.
2. **After refactor:** Agent calls `aether_verify_intent` to compare current SIR against snapshot.
3. **Result:** Symbols where intent was preserved vs. shifted, with specific before/after comparison and test coverage gap analysis.

#### MCP Tool: `aether_snapshot_intent`

**Request:**
```json
{
  "scope": "file",                    -- "file" | "symbol" | "directory"
  "target": "src/payments/processor.rs",
  "label": "pre-batch-refactor"       -- Human label for this snapshot
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "snapshot_id": "snap_a1b2c3",
  "label": "pre-batch-refactor",
  "symbols_captured": 8,
  "created_at": 1708000000000
}
```

**Storage:** Snapshots stored in SQLite `intent_snapshots` table:
```sql
CREATE TABLE IF NOT EXISTS intent_snapshots (
    snapshot_id     TEXT PRIMARY KEY,
    label           TEXT NOT NULL,
    scope           TEXT NOT NULL,     -- "file" | "symbol" | "directory"
    target          TEXT NOT NULL,     -- file path, symbol ID, or directory path
    symbols_json    TEXT NOT NULL,     -- JSON array of {symbol_id, sir_hash, sir_text, embedding}
    created_at      INTEGER NOT NULL
);
```

#### MCP Tool: `aether_verify_intent`

**Request:**
```json
{
  "snapshot_id": "snap_a1b2c3",
  "regenerate_sir": true              -- Re-run SIR generation on changed symbols before comparing
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "snapshot_id": "snap_a1b2c3",
  "label": "pre-batch-refactor",
  "verification": {
    "symbols_checked": 8,
    "intent_preserved": 6,
    "intent_shifted": 2,
    "symbols_removed": 0,
    "symbols_added": 1
  },
  "preserved": [
    {
      "symbol_id": "abc123",
      "symbol_name": "validate_order",
      "similarity": 0.97,
      "status": "preserved"
    }
  ],
  "shifted": [
    {
      "symbol_id": "def456",
      "symbol_name": "process_payment",
      "similarity": 0.62,
      "status": "shifted",
      "before_purpose": "Processes a single payment transaction with retry logic",
      "after_purpose": "Orchestrates batch payment processing with parallel gateway calls",
      "before_edge_cases": ["timeout after 3 retries", "negative amount rejected"],
      "after_edge_cases": ["batch size > 1000 triggers chunking", "partial batch failure returns partial results", "negative amount rejected"],
      "test_coverage_gap": {
        "existing_tests": ["handles timeout", "validates amount"],
        "untested_new_intents": ["batch size chunking", "partial batch failure handling"],
        "recommendation": "Add tests for batch-specific edge cases"
      }
    }
  ],
  "added": [
    {
      "symbol_id": "new789",
      "symbol_name": "batch_chunker",
      "file": "src/payments/processor.rs",
      "note": "New symbol not in original snapshot — verify test coverage"
    }
  ]
}
```

#### Intent Similarity Thresholds
```
similarity >= 0.90 → "preserved" (intent fundamentally unchanged)
similarity >= 0.70 → "shifted_minor" (intent adjusted but recognizable)
similarity < 0.70  → "shifted_major" (intent substantially changed)
```

### CLI Surface

```bash
# Graph health dashboard
aether health --limit 10 --min-risk 0.5

# Show critical symbols
aether health critical --top 10

# Show cycles
aether health cycles

# Show orphaned code
aether health orphans

# Intent verification workflow
aether snapshot-intent --file src/payments/processor.rs --label "pre-refactor"
# ... do refactor ...
aether verify-intent snap_a1b2c3 --regenerate-sir
```

### Config

```toml
[health]
enabled = true
risk_weights = { pagerank = 0.3, test_gap = 0.25, drift = 0.2, no_sir = 0.15, recency = 0.1 }

[intent]
enabled = true
similarity_preserved_threshold = 0.90
similarity_shifted_threshold = 0.70
auto_regenerate_sir = true      # Regenerate SIR for changed symbols before comparison
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| No dependency edges in CozoDB | Skip graph metrics, return empty results with note |
| Symbol removed during refactor | Listed in `symbols_removed` with its original SIR |
| New symbol added during refactor | Listed in `symbols_added`, not compared (no baseline) |
| Snapshot references symbol whose SIR was never generated | Skip symbol, note "no SIR at snapshot time" |
| Intent verification without prior snapshot | MCP error: "no snapshot found, use aether_snapshot_intent first" |
| CozoDB built-in algorithm not available | Fall back to manual implementation (PageRank iteration, Tarjan's SCC); log warning |
| Very large graph (>50K edges) | CozoDB handles this natively; add timeout (30s default) |
| Embeddings disabled | Graph health works (structure only); intent verification degrades to text diff |

### Pass Criteria
1. PageRank, community detection, betweenness centrality, SCC, and connected components queries execute correctly on CozoDB.
2. Risk score correctly combines pagerank, test coverage, drift, SIR presence, and access recency.
3. Risk hotspots surface symbols with multiple risk factors.
4. Cycle detection finds actual circular dependency chains.
5. Orphan detection identifies disconnected subgraphs.
6. `aether_snapshot_intent` captures SIR state for all symbols in scope.
7. `aether_verify_intent` correctly classifies preserved vs. shifted intents based on similarity threshold.
8. Test coverage gap analysis correctly identifies untested new edge cases.
9. New and removed symbols reported correctly.
10. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_8_graph_health_intent.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-8-graph-health-intent off main.
3) Create worktree ../aether-phase6-stage6-8 for that branch and switch into it.

PART A — Graph Health Metrics:
4) In crates/aether-analysis (or crates/aether-store):
   - Implement CozoDB graph queries:
     a. PageRank on dependency_edges.
     b. Louvain community detection.
     c. Betweenness centrality.
     d. Strongly connected components (cycle detection).
     e. Connected components (orphan detection).
   - Implement risk score computation:
     Cross-reference PageRank with test_coverage (from tested_by), drift_magnitude (from drift_results),
     SIR presence (from symbols), and access recency (from symbols.last_accessed_at).
   - Implement risk_factors human-readable explanation generation.
5) In crates/aether-mcp:
   - Add aether_health tool with request/response schema per spec.

PART B — Intent Verification:
6) In crates/aether-store:
   - Add intent_snapshots SQLite table and migration.
7) In crates/aether-analysis or crates/aether-memory:
   - Implement snapshot_intent: capture symbol SIR state (text + embedding) for scope.
   - Implement verify_intent:
     a. Load snapshot symbols.
     b. Get current SIR for each symbol (optionally regenerate via aether-infer).
     c. Compute cosine similarity between snapshot embedding and current embedding.
     d. Classify: preserved (>=0.90), shifted_minor (>=0.70), shifted_major (<0.70).
     e. For shifted symbols: compute SIR text diff (purpose, edge_cases fields).
     f. Cross-reference against test_intents: identify untested new edge cases.
8) In crates/aether-mcp:
   - Add aether_snapshot_intent tool.
   - Add aether_verify_intent tool.
9) Add CLI commands:
   - `aether health [critical|cycles|orphans] --limit <n> --min-risk <threshold>`
   - `aether snapshot-intent --file <path> --label <label>`
   - `aether verify-intent <snapshot_id> [--regenerate-sir]`
10) Add [health] and [intent] config sections.
11) Add tests:
    - Graph metrics on synthetic dependency graph with known PageRank, cycles, orphans.
    - Risk score computation with mocked metrics.
    - Snapshot + verify workflow: create snapshot, modify SIR, verify drift detected.
    - Test coverage gap: symbol with new edge cases not covered by tests.
    - Graceful degradation when no graph data or no embeddings.
12) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
13) Commit with message: "Add graph health metrics and intent verification"
```
