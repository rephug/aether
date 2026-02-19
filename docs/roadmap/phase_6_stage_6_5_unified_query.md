# Phase 6 — The Chronicler

## Stage 6.5 — Memory Search + Unified Query

### Purpose
Provide a single MCP tool that searches across ALL AETHER knowledge — symbols, SIR, project notes, coupling data, test intents — with unified ranking. Also enrich LSP hover with project context.

### What It Borrows From
- **Fold:** Single search endpoint across all content types
- **Zep:** Unified retrieval combining graph traversal + vector search + keyword matching

### MCP Tool: `aether_ask`

The "I don't know what I'm looking for" tool. Searches everything.

**Request:**
```json
{
  "query": "payment processing retry logic",
  "limit": 10,
  "include": ["symbols", "notes", "coupling", "tests"]  -- default: all
}
```

**Response:**
```json
{
  "schema_version": "1.0",
  "query": "payment processing retry logic",
  "result_count": 4,
  "results": [
    {
      "kind": "symbol",
      "id": "abc123",
      "title": "process_payment_with_retry",
      "snippet": "Processes payment with exponential backoff retry...",
      "relevance_score": 0.92,
      "file": "src/payments/processor.rs",
      "language": "rust"
    },
    {
      "kind": "note",
      "id": "x1y2z3",
      "title": null,
      "snippet": "Refactoring process_payment because the old approach was too slow...",
      "relevance_score": 0.85,
      "tags": ["refactor", "performance"],
      "source_type": "session"
    },
    {
      "kind": "test_guard",
      "id": "t1u2v3",
      "title": "retries on transient failure",
      "snippet": "it(\"should retry 3 times on timeout\")",
      "relevance_score": 0.78,
      "test_file": "src/payments/processor.test.ts"
    },
    {
      "kind": "coupled_file",
      "id": null,
      "title": "src/payments/gateway.rs",
      "snippet": "Co-changes with processor.rs in 89% of commits (Critical coupling, type: multi)",
      "relevance_score": 0.71,
      "fused_score": 0.89,
      "coupling_type": "multi"
    }
  ]
}
```

**Search implementation:**
1. Run hybrid search on symbols (existing `aether_search`).
2. Run hybrid search on project notes (`aether_recall` from 6.1).
3. Run text match on test intents.
4. For top symbol results, fetch coupled files from CozoDB.
5. Merge all results, normalize scores to [0, 1], apply RRF fusion across result types.
6. Apply recency + access boost.
7. Return top N.

### LSP Hover Enrichment

When hovering a symbol that has associated project notes or coupling data, append a concise section:

```markdown
## `process_payment`

**Purpose:** Processes payment transactions with retry logic...

**Dependencies:** calls `validate_order`, `gateway.charge`

---
📝 *"Refactored for streaming — old approach loaded all records into memory"* (2d ago)
⚠️ *Co-changes with gateway.rs (89%, Critical)*
🧪 *3 test guards: retries on timeout, handles negative balance, logs audit trail*
```

Rules:
- Show at most 1 note (most relevant), 1 coupling warning (highest risk), 3 test intents.
- Only show if data exists — no empty sections.
- Compact: one line per item.

### Pass Criteria
1. `aether_ask` returns mixed results from symbols, notes, coupling, and test intents.
2. Results are ranked by unified relevance score with cross-type RRF.
3. LSP hover shows project context when available.
4. LSP hover shows nothing extra when no notes/coupling/tests exist (no regression).
5. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

### Exact Codex Prompt
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_6_stage_6_5_unified_query.md for the full specification.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase6-stage6-5-unified-query off main.
3) Create worktree ../aether-phase6-stage6-5 for that branch and switch into it.
4) In crates/aether-memory or crates/aether-mcp:
   - Implement unified search: query symbols, notes, test intents, coupling in parallel.
   - Implement cross-type RRF score normalization and merging.
5) In crates/aether-mcp:
   - Add aether_ask tool with request/response schema per spec.
6) In crates/aether-lsp:
   - Enrich hover output with project notes, coupling warnings, test intents.
   - Respect compactness rules: 1 note, 1 coupling, 3 test intents max.
   - No extra sections when no context data exists.
7) Add CLI command:
   - `aether ask <query> --limit <n>`
8) Add tests:
   - Unified search returns mixed result types.
   - LSP hover includes context when available.
   - LSP hover regression: no change when no context data.
   - Cross-type ranking produces sensible ordering.
9) Run:
   - cargo fmt --all --check
   - cargo clippy --workspace -- -D warnings
   - cargo test --workspace
10) Commit with message: "Add unified query and LSP hover enrichment"
```
