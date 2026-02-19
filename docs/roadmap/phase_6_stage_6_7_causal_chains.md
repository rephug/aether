# Phase 6 — The Chronicler

## Stage 6.7 — Causal Change Chains

### Purpose
When something breaks, trace the *semantic* causal chain backward through dependencies to find which upstream change most likely caused it. `git blame` tells who changed a line. This tells you *which upstream semantic change* broke your downstream code — and *what specifically changed* about it.

**This is a genuinely novel capability.** Change Impact Graphs (2009, Orso et al.) propagated file-level changes through dependency graphs but had no semantic understanding — they couldn't tell *what* changed about a function's behavior. AETHER can say "validate_payment now rejects empty currency codes" because it has SIR diff.

### What It Requires
- **CozoDB dependency graph** (Phase 4): CALLS/DEPENDS_ON edges for backward traversal.
- **SIR versioning** (Phase 2): `sir_history` for detecting *what* changed semantically.
- **Multi-signal coupling** (Stage 6.2): fused_score for ranking candidates.
- **gix** (Phase 4): Commit timestamps for recency weighting.

### Algorithm

```
Input: target_symbol_id, lookback_window (default: "20 commits")

1. Get target symbol's current file and all direct + transitive upstream dependencies:
   upstream_symbols = CozoDB recursive Datalog query:
   ?[upstream, depth] :=
       dependency_edges[target_symbol_id, upstream, _], depth = 1
   ?[upstream, depth] :=
       upstream_symbols[mid, d], dependency_edges[mid, upstream, _], depth = d + 1, depth <= max_depth

2. For each upstream symbol:
   a. Query sir_history: did this symbol's SIR change within lookback_window?
   b. If yes:
      - Get before/after SIR text.
      - Compute SIR diff (structured comparison of purpose, edge_cases, dependencies fields).
      - Compute change_magnitude = 1.0 - cosine_similarity(before_embedding, after_embedding).
      - Get commit hash and timestamp of the change.

3. Rank upstream changes by:
   causal_score = recency_weight * coupling_strength * change_magnitude
   Where:
     recency_weight = 1.0 / (1.0 + days_since_change)
     coupling_strength = fused_score from co_change_edges (if exists), else 0.5 * (1.0 / depth)
     change_magnitude = SIR embedding distance (from step 2b)

4. Return top N candidates as causal chain, ordered by causal_score descending.
```

### MCP Tool: `aether_trace_cause`

**Request:**
```json
{
  "symbol": "process_payment",       -- symbol name or ID
  "file": "src/payments/processor.rs",  -- optional, helps disambiguate
  "lookback": "20 commits",
  "max_depth": 5,                    -- dependency graph traversal depth
  "limit": 5                         -- max candidates to return
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "target": {
    "symbol_id": "abc123",
    "symbol_name": "process_payment",
    "file": "src/payments/processor.rs"
  },
  "analysis_window": {
    "lookback": "20 commits",
    "max_depth": 5,
    "upstream_symbols_scanned": 23
  },
  "causal_chain": [
    {
      "rank": 1,
      "causal_score": 0.87,
      "symbol_id": "def456",
      "symbol_name": "validate_currency",
      "file": "src/payments/currency.rs",
      "dependency_path": ["process_payment", "validate_order", "validate_currency"],
      "depth": 2,
      "change": {
        "commit": "a1b2c3d",
        "author": "alice",
        "date": "2026-02-15T14:30:00Z",
        "change_magnitude": 0.42,
        "sir_diff": {
          "purpose_changed": true,
          "purpose_before": "Validates currency code is a recognized ISO 4217 value",
          "purpose_after": "Validates currency code is ISO 4217 AND not in sanctions blocklist",
          "edge_cases_added": ["Empty currency code now rejected (was previously defaulted to USD)"],
          "edge_cases_removed": []
        }
      },
      "coupling": {
        "fused_score": 0.63,
        "coupling_type": "multi"
      }
    },
    {
      "rank": 2,
      "causal_score": 0.54,
      "symbol_id": "ghi789",
      "symbol_name": "gateway_charge",
      "file": "src/payments/gateway.rs",
      "dependency_path": ["process_payment", "gateway_charge"],
      "depth": 1,
      "change": {
        "commit": "b2c3d4e",
        "author": "bob",
        "date": "2026-02-14T09:15:00Z",
        "change_magnitude": 0.28,
        "sir_diff": {
          "purpose_changed": false,
          "edge_cases_added": ["Now throws GatewayTimeoutError after 30s (was 60s)"],
          "edge_cases_removed": []
        }
      },
      "coupling": {
        "fused_score": 0.89,
        "coupling_type": "multi"
      }
    }
  ],
  "no_change_upstream": 21
}
```

### CLI Surface

```bash
# Trace cause of breakage
aether trace-cause process_payment --file src/payments/processor.rs --lookback "20 commits"

# Shorthand: trace from current file context
aether trace-cause --symbol-id abc123 --depth 3
```

### Edge Cases

| Scenario | Behavior |
|----------|----------|
| Target symbol has no dependencies | Return empty `causal_chain`, note: "no upstream dependencies" |
| No upstream SIR changes in window | Return empty `causal_chain`, note: "no semantic changes in window" |
| Circular dependency in graph | CozoDB handles cycles in recursive queries; `depth` limit prevents infinite traversal |
| Symbol not found | MCP error: "symbol not found, try aether_search to find it" |
| No SIR history for upstream symbol | Skip that symbol (can't compute diff), note in response |
| Very deep dependency chain (depth > 10) | Cap at max_depth, note "truncated at depth N" |

### Pass Criteria
1. CozoDB recursive Datalog query correctly traverses upstream dependencies to specified depth.
2. SIR diff correctly identifies changed purpose, edge_cases, and dependencies fields.
3. Change magnitude computed from embedding cosine similarity.
4. Causal score correctly combines recency, coupling, and change magnitude.
5. Results ordered by causal_score descending.
6. Dependency path shows actual traversal chain from target to upstream.
7. Graceful handling of cycles, missing SIR history, and zero-change windows.
8. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_7_causal_chains.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-7-causal-chains off main.
3) Create worktree ../aether-phase6-stage6-7 for that branch and switch into it.
4) In crates/aether-store or crates/aether-analysis:
   - Implement recursive upstream dependency query in CozoDB Datalog:
     Traverse CALLS/DEPENDS_ON edges backward from target symbol to max_depth.
   - Implement SIR diff: compare two SIR versions field-by-field (purpose, edge_cases, dependencies).
   - Implement change_magnitude from embedding cosine similarity (query LanceDB).
   - Implement causal_score = recency_weight * coupling_strength * change_magnitude.
   - Implement ranking and limit.
5) In crates/aether-mcp:
   - Add aether_trace_cause tool with request/response schema per spec.
   - Symbol resolution: accept name + file, or symbol_id directly.
6) Add CLI command:
   - `aether trace-cause <symbol_name> --file <path> --lookback <window> --depth <n>`
7) Add tests:
   - Synthetic dependency graph A→B→C with known SIR changes — verify correct causal chain.
   - Verify ranking: more recent + higher coupling + higher magnitude = higher score.
   - Verify cycle handling doesn't loop.
   - Verify graceful degradation with missing SIR history.
8) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
9) Commit with message: "Add causal change chain tracing with SIR diff"
```
