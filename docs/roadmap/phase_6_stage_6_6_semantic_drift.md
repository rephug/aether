# Phase 6 — The Chronicler

## Stage 6.6 — Semantic Drift Detection

### Purpose
Automatically detect when code's *meaning* is changing incrementally — without anyone explicitly noting it — by comparing current SIR against historical SIR. Also detect architectural boundary violations by running community detection on the dependency graph and flagging new cross-community edges.

**This is a genuinely novel capability.** Architectural drift detection in existing tools (ArchUnit, jQAssistant) requires manually maintained architecture models. AETHER detects drift automatically from the code's own semantic history because it has versioned SIR annotations per symbol — something no other system maintains.

### What It Requires
- **SIR versioning** (Phase 2): `sir_history` table with previous SIR versions per symbol.
- **LanceDB embeddings** (Phase 4): SIR embeddings for computing semantic similarity over time.
- **CozoDB graph** (Phase 4): Dependency edges for community detection.
- **gix** (Phase 4): Commit range for bounding drift analysis.

### Drift Detection Algorithm

```
Semantic Drift (per-symbol):
1. For each symbol with SIR:
   a. Get current SIR embedding from LanceDB.
   b. Get SIR embedding from N commits ago (from sir_history, find sir_version closest to target commit).
   c. Compute cosine similarity between current and historical embedding.
   d. If similarity < drift_threshold (default 0.85): flag as "drifted."
   e. Record drift magnitude = 1.0 - similarity.
   f. Record drift period: commit range over which drift accumulated.

Boundary Violation Detection:
1. Run CozoDB Louvain community detection on the full dependency graph:
   ?[node, community] := community_detection_louvain(*dependency_edges[], node, community)
2. For each CALLS/DEPENDS_ON edge where source community ≠ target community:
   a. Check if this cross-community edge existed N commits ago (query sir_history for edge existence).
   b. If edge is NEW since the analysis window: flag as boundary violation.

Structural Anomaly Detection:
1. Hub detection: CozoDB PageRank on dependency graph. Symbols with PageRank > 95th percentile AND PageRank increased by >20% since last analysis = "emerging god objects."
2. Cycle detection: CozoDB SCC (strongly connected components) query. New cycles that didn't exist N commits ago = "emerging circular dependencies."
3. Orphan detection: connected components query. Subgraphs with no edge to the main component = "orphaned code candidates."
```

### Schema

**SQLite: `drift_analysis_state` table**
```sql
CREATE TABLE IF NOT EXISTS drift_analysis_state (
    id                  INTEGER PRIMARY KEY DEFAULT 1,
    last_analysis_commit TEXT,
    last_analysis_at    INTEGER,
    symbols_analyzed    INTEGER DEFAULT 0,
    drift_detected      INTEGER DEFAULT 0
);
```

**SQLite: `drift_results` table**
```sql
CREATE TABLE IF NOT EXISTS drift_results (
    result_id           TEXT PRIMARY KEY,   -- BLAKE3(symbol_id + analysis_commit)
    symbol_id           TEXT NOT NULL,
    file_path           TEXT NOT NULL,
    symbol_name         TEXT NOT NULL,
    drift_type          TEXT NOT NULL,      -- "semantic" | "boundary_violation" | "emerging_hub" | "new_cycle" | "orphaned"
    drift_magnitude     REAL,              -- 0.0-1.0 for semantic drift, null for structural
    current_sir_hash    TEXT,
    baseline_sir_hash   TEXT,
    commit_range_start  TEXT,
    commit_range_end    TEXT,
    detail_json         TEXT NOT NULL,      -- JSON with type-specific details
    detected_at         INTEGER NOT NULL,
    is_acknowledged     INTEGER NOT NULL DEFAULT 0  -- User can acknowledge/dismiss
);

CREATE INDEX idx_drift_results_type ON drift_results(drift_type);
CREATE INDEX idx_drift_results_file ON drift_results(file_path);
CREATE INDEX idx_drift_results_ack ON drift_results(is_acknowledged);
```

### MCP Tool: `aether_drift_report`

**Request:**
```json
{
  "window": "50 commits",           -- or "30d" or "since:a1b2c3d"
  "include": ["semantic", "boundary", "structural"],  -- default: all
  "min_drift_magnitude": 0.15,      -- only for semantic drift
  "include_acknowledged": false
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "analysis_window": {
    "from_commit": "a1b2c3d",
    "to_commit": "e4f5g6h",
    "commit_count": 50,
    "analyzed_at": 1708000000000
  },
  "summary": {
    "symbols_analyzed": 342,
    "semantic_drifts": 5,
    "boundary_violations": 2,
    "emerging_hubs": 1,
    "new_cycles": 0,
    "orphaned_subgraphs": 1
  },
  "semantic_drift": [
    {
      "symbol_id": "abc123",
      "symbol_name": "process_payment",
      "file": "src/payments/processor.rs",
      "drift_magnitude": 0.31,
      "similarity": 0.69,
      "drift_summary": "Function's purpose shifted from single-payment processing to batch payment orchestration over 12 commits",
      "commit_range": ["a1b2c3d", "e4f5g6h"],
      "test_coverage": {
        "has_tests": true,
        "test_count": 3,
        "intents": ["handles timeout", "validates amount", "logs transaction"]
      }
    }
  ],
  "boundary_violations": [
    {
      "source_symbol": "validate_order",
      "source_file": "src/orders/validator.rs",
      "source_community": 3,
      "target_symbol": "charge_card",
      "target_file": "src/payments/gateway.rs",
      "target_community": 7,
      "edge_type": "CALLS",
      "first_seen_commit": "c3d4e5f",
      "note": "New cross-module dependency: orders module now directly calls payments module"
    }
  ],
  "structural_anomalies": {
    "emerging_hubs": [
      {
        "symbol_id": "def456",
        "symbol_name": "AppContext",
        "file": "src/context.rs",
        "current_pagerank": 0.94,
        "previous_pagerank": 0.71,
        "dependents_count": 47,
        "note": "PageRank increased 32% — becoming a god object"
      }
    ],
    "new_cycles": [],
    "orphaned_subgraphs": [
      {
        "symbols": ["old_parser", "legacy_format"],
        "files": ["src/legacy/parser.rs"],
        "total_symbols": 2,
        "note": "No dependency edges to main application — dead code candidate"
      }
    ]
  }
}
```

### MCP Tool: `aether_acknowledge_drift`

**Request:**
```json
{
  "result_ids": ["r1", "r2"],
  "note": "Intentional — process_payment was deliberately expanded to handle batches per ADR-23"
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "acknowledged": 2,
  "note_created": true,
  "note_id": "n1a2b3..."
}
```

Acknowledged drift items get stored as project notes (6.1) and excluded from future reports unless `include_acknowledged: true`.

### CLI Surface

```bash
# Run drift analysis
aether drift-report --window "50 commits" --min-drift 0.15

# Acknowledge drift items
aether drift-ack <result_id> --note "Intentional per ADR-23"

# Show community structure
aether communities --format table
```

### Config

```toml
[drift]
enabled = true
drift_threshold = 0.85          # Cosine similarity below this = drift
analysis_window = "100 commits" # Default analysis window
auto_analyze = false            # Run drift analysis on every indexing pass
hub_percentile = 95             # PageRank percentile for hub detection
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Symbol has no SIR history (new symbol) | Skip — no baseline to compare against |
| Symbol's SIR was regenerated (not changed) | Same embedding → similarity ≈ 1.0 → no false positive |
| Fewer commits than window | Analyze all available, note "limited_history" |
| No dependency edges in CozoDB | Skip community/structural analysis, run semantic drift only |
| Embeddings disabled | Skip semantic drift, run structural analysis only |
| Very large codebase (>10K symbols) | Batch embedding comparisons; limit to symbols changed in window |

### Pass Criteria
1. Semantic drift detection flags symbols whose SIR embedding similarity dropped below threshold.
2. Community detection runs via CozoDB Louvain and identifies module boundaries.
3. Boundary violations correctly identify new cross-community dependency edges.
4. Hub detection flags symbols with rising PageRank above percentile threshold.
5. Cycle detection finds new strongly connected components.
6. Orphan detection identifies disconnected subgraphs.
7. `aether_acknowledge_drift` suppresses items from future reports and creates project notes.
8. Graceful degradation when embeddings or SIR history unavailable.
9. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_6_semantic_drift.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-6-semantic-drift off main.
3) Create worktree ../aether-phase6-stage6-6 for that branch and switch into it.
4) In crates/aether-store:
   - Add drift_analysis_state and drift_results SQLite tables and migrations.
   - Add query helpers: get SIR embedding at commit N for a symbol (join sir_history + LanceDB).
5) In crates/aether-memory or new module crates/aether-analysis:
   - Implement semantic drift detection:
     a. For symbols changed in window, get current embedding and baseline embedding.
     b. Compute cosine similarity, flag if below threshold.
   - Implement boundary violation detection:
     a. Run CozoDB Louvain community detection.
     b. Identify cross-community CALLS/DEPENDS_ON edges not present in baseline.
   - Implement structural anomaly detection:
     a. PageRank for hub detection (compare current vs. baseline percentile).
     b. SCC for new cycle detection.
     c. Connected components for orphan detection.
6) In crates/aether-mcp:
   - Add aether_drift_report tool with request/response schema per spec.
   - Add aether_acknowledge_drift tool that marks items acknowledged and creates project note.
7) Add CLI commands:
   - `aether drift-report --window <window> --min-drift <threshold>`
   - `aether drift-ack <result_id> --note <text>`
   - `aether communities --format table`
8) Add [drift] config section in aether-config.
9) Add tests:
   - Synthetic SIR history with known drift — verify detection.
   - Synthetic dependency graph with known communities — verify boundary violation detection.
   - Hub detection with synthetic PageRank data.
   - Acknowledge flow: drift item → acknowledged → excluded from report.
   - Graceful degradation when no SIR history or no embeddings.
10) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
11) Commit with message: "Add semantic drift detection with boundary and structural analysis"
```
