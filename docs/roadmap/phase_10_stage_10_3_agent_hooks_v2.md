# Phase 10 — The Conductor

## Stage 10.3 — Agent Integration Hooks

### Purpose

Expose AETHER's semantic intelligence as CLI commands that AI coding agents (Claude Code, Codex, Cursor) can call programmatically. Stage 5.6 created the `init-agent` scaffolding and CLAUDE.md templates. This stage adds the actual runtime commands those templates reference.

### What Problem This Solves

Today, AI agents access AETHER via MCP tools (when MCP is running) or by reading `.aether/sirs/*.json` files (which requires knowing the file layout). Two gaps:

1. **Context injection:** An agent starting a task needs the full semantic context for a set of symbols — SIR intents, dependency edges, coupling data, test intents, project memory notes — assembled into a single text block it can consume in one shot. Currently the agent must call 5+ separate MCP tools and stitch the results together.

2. **SIR feedback:** After an agent generates or modifies code, it has no way to tell AETHER "I changed this symbol's intent to X" without waiting for the next file-save reindex. Direct injection lets the agent contribute its understanding immediately.

### In scope

#### `aetherd sir context <symbol_selector>`

Assembles a comprehensive context block for one or more symbols. Output is a single text document (stdout or file) containing all relevant intelligence for the target symbol(s).

**Token budget knapsack (Deep Think finding F1):**

Context assembly uses a greedy fractional knapsack with `--max-tokens` (default 16,000). Token estimation: `chars / 3.5`. Items are inserted in strict priority order:

| Priority | Content | Rationale |
|----------|---------|-----------|
| 1 (must-have) | Target symbol source + SIR | Agent needs the actual code and its semantic annotation |
| 2 | Test intents | Highest signal-to-noise — tells the agent what behavior is guarded |
| 3 | 1-hop dependency intents | What the symbol depends on (intents only, not full source) |
| 4 | 1-hop caller signatures | What calls this symbol (signatures only, compact) |
| 5 | Coupling data | Temporal, semantic, structural coupling signals |
| 6 | Project memory notes | Design decisions, rationale |
| 7 | Recent git changes | Last 5 commits touching the file |
| 8 | Health/staleness scores | Current quality and freshness signals |
| 9 | 2-hop transitive deps | Only if budget allows |

If budget is exhausted mid-tier, truncate and append:
```
> [Context truncated: 14 transitive dependencies omitted to fit budget]
```

The agent knows it's getting a partial view and can request a higher budget if needed.

Flags:
- `--format <text|json|markdown>` — output format (default: markdown)
- `--max-tokens <N>` — token budget (default: 16000)
- `--depth <1-3>` — how many hops of dependency context to include (default: 1)
- `--include <deps,dependents,coupling,tests,memory,changes,health>` — comma-separated sections to include (default: all)
- `--output <path>` — write to file instead of stdout
- `--symbols <file>` — read symbol selectors from a file (one per line) for batch context assembly

Selector format: qualified name (`my_crate::module::function_name`), symbol ID, or fuzzy search string.

#### `aetherd sir inject <symbol_selector> --intent "<text>"`

Directly set or update a symbol's SIR intent without running inference. Use cases:

- Agent just generated code and knows exactly what it does
- Human developer wants to annotate a symbol manually
- Batch correction of SIR quality issues found during review

Behavior:
1. Find the symbol by selector
2. Load the existing SIR (or create a new one if none exists)
3. Update the `intent` field with the provided text
4. Set `generation_pass = "injected"` and `generation_model = "manual"`
5. Update `sir_generated_at` to now
6. Persist to SQLite and mirror to `.aether/sirs/` if `mirror_sir_files = true`
7. **Synchronous re-embed (Deep Think finding F2):** If `[embeddings] enabled = true`, immediately re-embed the updated SIR using the configured embedding provider. For Gemini Embedding 2 this is one API call (~100ms). For local Ollama, ~50ms. This guarantees read-after-write consistency — an agent that injects an intent and immediately runs a semantic search will find the updated symbol.
8. Update `prompt_hash` to reflect the new state (ensures batch pipeline doesn't overwrite the injection unless context actually changes further)
9. **Write fingerprint history row:** Record the injection in `sir_fingerprint_history` with `trigger = "inject"`, `source_changed = 0`, `neighbor_changed = 0`, `config_changed = 0`, and Δ_sem computed from old vs new embedding. This creates an audit trail for agent-authored SIRs.

Flags:
- `--intent "<text>"` — the intent string (required)
- `--behavior "<text>"` — optional behavior summary
- `--edge-cases "<text>"` — optional edge case notes
- `--force` — overwrite even if existing SIR has higher quality score
- `--dry-run` — show what would change without persisting
- `--no-embed` — skip re-embedding (for environments without embedding provider configured)

#### `aetherd sir diff <symbol_selector>`

Show the delta between the current SIR and what the source code currently implies. Useful for agents to check "is the SIR still accurate for what I'm about to change?"

Behavior:
1. Load current SIR
2. Re-parse the symbol's source code via tree-sitter
3. Run a lightweight structural comparison:
   - Function signature changed? (parameters, return type)
   - New error paths? (Result/Option return where there wasn't one)
   - Visibility changed? (pub → pub(crate), etc.)
   - Body complexity changed significantly? (line count delta > 50%)
   - Dependencies changed? (new imports, removed calls)
4. Output a structured diff showing what's stale

This does NOT run inference — it's a fast structural comparison only. An agent can use this to decide whether to call `sir inject` with a manual update or wait for the watcher to re-index.

### Out of scope

- MCP tool wrappers for these commands (MCP tools already cover most use cases; these CLI commands are for non-MCP agent workflows)
- Web API endpoints (CLI-only; dashboard integration is Phase 9)
- Bulk inject from JSONL (could be added later)
- SIR quality scoring for injected intents (injected SIRs get `confidence = 0.5` by default)

### Implementation Notes

#### CLI wiring

Add to `Commands` enum in `cli.rs`:

```rust
/// Assemble semantic context for a symbol
SirContext(SirContextArgs),
/// Inject or update a symbol's SIR intent
SirInject(SirInjectArgs),
/// Show SIR vs source code delta
SirDiff(SirDiffArgs),
```

All three commands need read access to SQLite (for symbols, SIR, edges, test intents, project notes). `sir context` uses SQLite `symbol_edges` for dependency/dependent lookups (NOT SurrealDB — avoids lock contention with the daemon). Coupling data requires SurrealDB access which may be unavailable if the daemon is running — handle gracefully. `sir inject` additionally needs write access to SQLite and optionally the embedding provider. They do NOT need inference providers or the watcher running.

#### Context assembly — markdown template

The markdown format is designed to be copy-pasted into an AI agent's context window:

```markdown
# Symbol: my_crate::payments::validate_amount

**Kind:** Function | **File:** src/payments.rs:42-78 | **Staleness:** 0.12

## Intent
Validates that a payment amount is positive, within the account's daily limit,
and does not exceed the remaining balance. Returns `PaymentError::InsufficientFunds`
if balance check fails.

## Dependencies (1 hop)
- `account::get_balance()` — Returns current available balance
- `limits::daily_limit()` — Returns configured daily transaction cap
- `types::PaymentError` — Error enum for payment failures

## Dependents
- `payments::process_payment()` — Calls this before executing transfer
- `api::handle_payment_request()` — HTTP handler, calls via process_payment

## Coupling
- `account::get_balance` — temporal 0.82, semantic 0.71 (co-changes in 14/17 commits)

## Test Guards
- `test_validate_amount_zero` — "should reject zero amount"
- `test_validate_amount_over_limit` — "should reject amounts exceeding daily limit"

## Recent Changes
- 2 days ago: Added daily limit check (commit abc123)
- 2 weeks ago: Changed error type from String to PaymentError (commit def456)

> [Context budget: 3,847 / 16,000 tokens used]
```

#### Context assembly — data sources

| Section | Source |
|---------|--------|
| Symbol metadata | SQLite `symbols` table |
| SIR intent/behavior | SQLite `sir` table (`sir_json` column) |
| Dependencies | SQLite `symbol_edges` via `store.get_dependencies()` |
| Dependents | SQLite `symbol_edges` via `store.get_callers()` |
| Coupling | SurrealDB via `list_co_change_edges_for_file()` — handle lock gracefully |
| Test intents | SQLite `test_intents` table |
| Project memory | SQLite `project_notes` table |
| Git changes | `gix` blame/log for the symbol's file |
| Health/staleness | SQLite `sir.staleness_score`, `aether-health` compute |

### Pass criteria

1. `aetherd sir context payments::validate_amount` outputs a complete context block with SIR, dependencies, dependents, and coupling data.
2. `--format json` produces valid JSON containing all the same sections.
3. `--max-tokens 4000` truncates lower-priority sections and appends the truncation notice.
4. `--depth 2` includes transitive dependencies (dependencies of dependencies).
5. `--include deps,tests` outputs only the dependencies and test intent sections.
6. `aetherd sir inject payments::validate_amount --intent "Validates payment amounts"` updates the SIR in SQLite and triggers synchronous re-embedding.
7. After inject, `sir context` shows the updated intent with `generation_pass = "injected"`.
8. After inject, semantic search finds the symbol using terms from the new intent.
9. `--dry-run` shows the change without persisting.
10. `--no-embed` skips re-embedding (verify no embedding API call in logs).
11. `aetherd sir diff` correctly identifies when a function's signature has changed since its last SIR.
12. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings` pass.
13. `cargo test -p aetherd` passes.

### Estimated Codex runs: 1–2
