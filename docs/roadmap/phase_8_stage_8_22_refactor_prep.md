# Phase 8.22: Refactor-Prep Deep Scan

## Purpose

Add a CLI subcommand that prepares a crate or file for safe refactoring
by running targeted Sonnet-quality deep passes on the symbols that matter
most — identified by AETHER's own health reports, graph centrality, and
risk scores. The output is a refactoring brief: a structured document
containing ★★★★★ SIR for every critical symbol in scope, dependency
paths that must be preserved, and a pre-refactor intent snapshot for
post-refactor verification.

This is the "AETHER tells you what you'd break" feature. A blind LLM
reads source code. AETHER tells you which 25 of 200 symbols have subtle
behavior that will silently break if moved wrong — and exactly what that
behavior is.

## Prerequisites

- Phase 8.18 merged (Boundary Leaker fix — cleaner health scores)
- Three-pass pipeline operational (scan/triage/deep)
- Health scoring operational (structural + semantic)
- Intent snapshot concept from Phase 6.8 (can be implemented minimally here)

## What Changes

### 1. New CLI subcommand: `aether refactor-prep`

```bash
# Prep a single file
aether refactor-prep --file crates/aether-mcp/src/lib.rs

# Prep an entire crate
aether refactor-prep --crate aether-mcp

# Control depth
aether refactor-prep --crate aether-mcp --top-n 30

# Use local model instead of cloud (cheaper, lower quality)
aether refactor-prep --crate aether-mcp --local

# Output as JSON for programmatic consumption
aether refactor-prep --crate aether-mcp --output json
```

**Defaults:**
- `--top-n 20` — deep-scan the 20 highest-priority symbols
- Cloud provider (Sonnet via OpenRouter) unless `--local` specified
- Human-readable output unless `--output json`

### 2. Symbol selection algorithm

The selection pipeline uses existing health infrastructure to pick the
symbols that matter most for safe refactoring:

```
Input: all symbols in scope (file or crate)
  │
  ├─ 1. Load existing health report (or compute if stale)
  │     → critical_symbols (pagerank + betweenness)
  │     → bottlenecks (high betweenness centrality)
  │     → risk_hotspots (composite risk score)
  │     → cycle_members (symbols in dependency cycles)
  │
  ├─ 2. Score each symbol for "refactoring risk"
  │     refactor_risk = 
  │       0.30 × pagerank_normalized        # how much depends on this
  │     + 0.25 × betweenness_normalized     # how many paths flow through this
  │     + 0.20 × (1.0 - test_coverage)      # untested = higher risk
  │     + 0.15 × blast_radius_normalized    # how far does breakage spread
  │     + 0.10 × cross_community_edges      # boundary symbols need more care
  │
  ├─ 3. Force-include cycle members
  │     Any symbol participating in a dependency cycle is automatically
  │     included regardless of rank — cycles are the #1 source of
  │     subtle refactoring breakage.
  │
  ├─ 4. Take top-N by refactor_risk score
  │     Default N=20. Configurable via --top-n.
  │
  └─ Output: ordered list of symbol IDs to deep-scan
```

### 3. Targeted deep pass execution

For each selected symbol:

1. Check if a deep-pass SIR already exists and is fresh
   (same signature fingerprint, `generation_pass = "deep"`).
   If so, skip — don't waste API calls on symbols that already
   have ★★★★★ SIR.

2. If no fresh deep SIR exists, run the deep pass:
   - Assemble enriched context: source text + neighbor intents +
     existing triage SIR as baseline
   - Call the deep provider (default: Sonnet via OpenRouter)
   - Store the result with `generation_pass = "deep"`

3. Track cost: log token count and estimated cost per symbol.
   Print summary at end.

### 4. Intent snapshot

After deep passes complete, snapshot the current SIR state for all
symbols in scope (not just the deep-scanned ones). This snapshot is
the "before" for post-refactor verification.

```rust
pub struct IntentSnapshot {
    /// Unique ID for this snapshot
    pub snapshot_id: String,
    /// Git commit at time of snapshot
    pub git_commit: String,
    /// Timestamp
    pub created_at: i64,
    /// Scope description (file path or crate name)
    pub scope: String,
    /// Per-symbol SIR + fingerprint at snapshot time
    pub symbols: Vec<SnapshotEntry>,
}

pub struct SnapshotEntry {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub signature_fingerprint: String,
    pub sir_json: String,
    pub generation_pass: String,  // "scan", "triage", or "deep"
    pub was_deep_scanned: bool,   // true if this run upgraded it
}
```

Storage: SQLite table `intent_snapshots` + `intent_snapshot_entries`.
One snapshot per `refactor-prep` run. Retained until explicitly pruned.

### 5. Refactoring brief output

Human-readable output (default):

```
══════════════════════════════════════════════════════════
  AETHER Refactoring Brief
  Scope: crates/aether-mcp/src/lib.rs (4799 lines)
  Commit: a1b2c3d
  Deep-scanned: 22 symbols (3 skipped — already fresh)
  Cost: $0.44 (19 symbols × ~$0.023)
  Snapshot: snap_1710412800_aether-mcp
══════════════════════════════════════════════════════════

── Critical Symbols (handle with extreme care) ──────────

  1. McpServer::handle_tool_call
     Risk: 0.91 | PageRank: 0.87 | Betweenness: 0.79
     Intent: Dispatches incoming MCP tool requests to
       registered handlers, managing request validation,
       error normalization across 14 tool endpoints, and
       structured JSON-RPC response formatting with
       timeout enforcement per tool.
     Side effects: Acquires shared workspace lock,
       may trigger SurrealDB queries, writes to tracing span
     Dependencies: SharedState, ToolRouter, all 14 tool
       handler functions, serde_json, tower timeout
     ⚠ 6 error propagation paths — 2 are silent
       (timeout returns empty result, not error)
     ⚠ Bottleneck: 79% of MCP request paths flow through here

  2. SharedState::validate_workspace
     Risk: 0.84 | PageRank: 0.72 | Betweenness: 0.68
     ...

── Dependency Cycles (all members must move together) ────

  Cycle 1: handle_tool_call → validate_workspace →
           ensure_indexed → handle_tool_call
  ⚠ These 3 symbols form a cycle. Moving any one without
    the others will break the call chain.

── Dependency Paths to Preserve ──────────────────────────

  handle_tool_call → [tool handlers] → SharedState
  All 14 tool handlers depend on SharedState.
  If SharedState moves to a different module, all handlers
  must be updated or SharedState must be re-exported.

── Intent Snapshot Saved ─────────────────────────────────

  ID: snap_1710412800_aether-mcp
  Symbols captured: 247 (22 at deep quality, 225 at triage)

  After refactoring, run:
    aether verify-intent --snapshot snap_1710412800_aether-mcp
  to check for unintended semantic drift.
```

JSON output (`--output json`): same data as structured JSON for
programmatic consumption by Codex prompts or MCP tools.

### 6. Post-refactor verification: `aether verify-intent`

Companion subcommand that compares current SIR against a saved snapshot:

```bash
aether verify-intent --snapshot snap_1710412800_aether-mcp
```

For each symbol in the snapshot:
1. Re-generate SIR (or use existing if fingerprint unchanged)
2. Compute semantic similarity between old and new intent
3. Flag symbols where similarity drops below threshold (default 0.85)
4. Report symbols that disappeared (deleted/renamed)
5. Report new symbols not in snapshot

Output: pass/fail with specific "intent drift" callouts.

### 7. MCP tool: `aether_refactor_prep`

Expose the same functionality as an MCP tool so Codex can call it
directly during planning:

```json
{
  "name": "aether_refactor_prep",
  "description": "Prepare a file or crate for refactoring by deep-scanning critical symbols and creating an intent snapshot",
  "input_schema": {
    "type": "object",
    "properties": {
      "scope": {
        "type": "string",
        "description": "File path or crate name to prepare"
      },
      "top_n": {
        "type": "integer",
        "default": 20,
        "description": "Number of symbols to deep-scan"
      }
    },
    "required": ["scope"]
  }
}
```

Returns: JSON refactoring brief + snapshot ID.

### 8. MCP tool: `aether_verify_intent`

```json
{
  "name": "aether_verify_intent",
  "description": "Compare current SIR against a saved intent snapshot to detect unintended semantic changes",
  "input_schema": {
    "type": "object",
    "properties": {
      "snapshot_id": { "type": "string" }
    },
    "required": ["snapshot_id"]
  }
}
```

Returns: per-symbol pass/fail with similarity scores and drift details.

## Implementation

### New files

```
crates/aetherd/src/refactor_prep.rs    — CLI orchestration
crates/aether-health/src/refactor.rs   — symbol selection + risk scoring
crates/aether-store/src/snapshots.rs   — intent snapshot persistence
crates/aether-mcp/src/tools/refactor.rs — MCP tools
```

### Modified files

```
crates/aetherd/src/main.rs             — add refactor-prep and verify-intent subcommands
crates/aether-store/src/lib.rs         — add snapshot sub-trait + re-export
crates/aether-store/src/schema.rs      — migration for intent_snapshots tables
crates/aether-mcp/src/tools/router.rs  — register new tools
crates/aether-mcp/src/tools/mod.rs     — declare refactor module
```

### Schema migration

```sql
CREATE TABLE IF NOT EXISTS intent_snapshots (
    snapshot_id   TEXT PRIMARY KEY,
    git_commit    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    scope         TEXT NOT NULL,
    symbol_count  INTEGER NOT NULL,
    deep_count    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS intent_snapshot_entries (
    snapshot_id           TEXT NOT NULL,
    symbol_id             TEXT NOT NULL,
    qualified_name        TEXT NOT NULL,
    file_path             TEXT NOT NULL,
    signature_fingerprint TEXT NOT NULL,
    sir_json              TEXT NOT NULL,
    generation_pass       TEXT NOT NULL,
    was_deep_scanned      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (snapshot_id, symbol_id),
    FOREIGN KEY (snapshot_id) REFERENCES intent_snapshots(snapshot_id)
);
```

## Scope Guards

- Do NOT modify the three-pass pipeline logic in sir_pipeline.rs
- Do NOT modify health scoring formulas
- Do NOT modify community detection or planner logic
- Do NOT modify existing MCP tools
- Do NOT modify Store sub-traits (add new SnapshotStore sub-trait)
- The deep pass uses the existing `generate_sir_with_retries` infra
- Symbol selection consumes health report output; does not recompute health

## A/B Test Integration

For the upcoming Store trait refactor A/B test, the workflow becomes:

**Blind run:** Codex reads source files, produces refactoring plan.

**AETHER-informed run:**
1. `aether refactor-prep --crate aether-store --top-n 25`
2. Codex receives the refactoring brief via MCP tool call
3. Codex produces refactoring plan informed by ★★★★★ SIR on
   the 25 riskiest symbols, dependency cycle warnings, and
   bottleneck identification
4. After refactoring: `aether verify-intent --snapshot <id>`
5. Verification report confirms no unintended semantic drift

The showcase story:
> AETHER identified 25 symbols out of 248 that carry the highest
> refactoring risk. It deep-scanned each with Claude Sonnet,
> discovering 6 silent error propagation paths and 2 dependency
> cycles that a blind refactoring would have broken. After the
> refactor, intent verification confirmed zero semantic drift.
> Cost: $0.58 and 3 minutes.

## Cost Estimate

- 20 symbols × $0.023/symbol (Sonnet) = $0.46
- Wall time: ~20 symbols × 17s = ~6 minutes (sequential)
  or ~2 minutes with concurrency 4
- Snapshot storage: negligible (JSON blobs in SQLite)

With `--local` flag: $0.00 but ★★½ quality (qwen3.5:4b can't
leverage enrichment effectively — Decision #57).

## Pass Criteria

1. `aether refactor-prep --file <path>` selects top-N symbols by
   refactoring risk, skipping symbols with fresh deep SIR.
2. Deep pass runs only on symbols that need upgrading. Token count
   and cost estimate printed.
3. Intent snapshot persisted to SQLite with full SIR for all in-scope
   symbols.
4. `aether verify-intent --snapshot <id>` compares current state
   against snapshot, reports per-symbol similarity with pass/fail.
5. `aether_refactor_prep` MCP tool returns JSON brief + snapshot ID.
6. `aether_verify_intent` MCP tool returns structured comparison.
7. Human-readable output includes critical symbols, cycles,
   bottlenecks, and dependency paths.
8. `--local` flag uses local provider instead of cloud.
9. `cargo fmt --all --check`, per-crate clippy and tests pass.

## Decisions

| # | Decision | Resolution | Rationale |
|---|----------|------------|-----------|
| 97 | Default deep provider for refactor-prep | Sonnet via OpenRouter | Proven ★★★★★ quality on Subscribe::apply benchmark. Cost is negligible for 20 symbols. |
| 98 | Default top-n | 20 | Covers critical symbols + bottlenecks + cycle members for typical God File. Configurable up to 100. |
| 99 | Intent snapshot storage | SQLite (same meta.sqlite) | Consistent with all other AETHER metadata. No new storage backend. |
| 100 | Verify-intent similarity threshold | 0.85 cosine | Below 0.85 between old and new intent embedding = likely semantic change. Conservative — better to flag false positives than miss real drift. |
| 101 | Skip fresh deep SIR | Yes, by signature fingerprint match | If fingerprint hasn't changed and generation_pass="deep", the existing SIR is still valid. Don't waste API calls. |

## End-of-Stage Git Sequence

```bash
git push -u origin feature/phase8-stage8-22-refactor-prep
```

**PR title:** Phase 8.22 — Refactor-Prep Deep Scan + Intent Verification

**PR body:**
Adds `aether refactor-prep` and `aether verify-intent` CLI subcommands plus
corresponding MCP tools. Refactor-prep uses health reports and graph centrality
to identify the highest-risk symbols in a file or crate, runs targeted Sonnet
deep passes on them, and saves an intent snapshot for post-refactor verification.

Key additions:
- Symbol selection by refactoring risk score (pagerank, betweenness, test coverage, blast radius, cross-community edges)
- Targeted deep pass execution with skip-if-fresh logic
- Intent snapshot persistence (new SQLite migration)
- Refactoring brief output (human-readable + JSON)
- `aether verify-intent` for post-refactor semantic drift detection
- `aether_refactor_prep` and `aether_verify_intent` MCP tools

```bash
# After merge:
git switch main
git pull --ff-only
git worktree remove <path>
git branch -d feature/phase8-stage8-22-refactor-prep
```
