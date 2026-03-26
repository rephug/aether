# Phase CC — Full SIR Inject: Agent-Authored Intelligence

**Codename:** The Scribe
**Depends on:** Phase 10.3 (sir inject CLI + MCP tool must exist)
**Modified crates:** `aetherd` (CLI args), `aether-mcp` (MCP request/tool), `aether-sir` (no schema change — struct already has all fields)
**New crates:** None
**Estimated Codex runs:** 1

---

## Thesis

Coding agents (Claude Code, Codex, Cursor) that create or modify code have *perfect knowledge* of what that code does — they just wrote it. The current `sir inject` tool only accepts `--intent`, `--behavior`, and `--edge-cases`, forcing agents to leave the richest SIR fields (`side_effects`, `dependencies`, `error_modes`, `complexity`, `confidence`) empty or at defaults. This wastes the agent's knowledge and produces hollow SIR records.

This stage upgrades `sir inject` (both CLI and MCP) to accept the **complete `SirAnnotation` schema**, and changes `generation_model` from the hardcoded `"manual"` to the actual model name (e.g., `claude-opus-4-6`, `codex-mini-latest`). This lets AETHER track *who* authored each SIR and at what quality tier.

**The payoff:** Every file a coding agent touches gets Opus/Sonnet-quality SIR at zero additional inference cost. The `generation_pass = "injected"` and `generation_model = "claude-opus-4-6"` fields create an auditable provenance trail. Downstream features (search ranking, drift detection, health scoring, contract verification) all benefit from richer SIR data.

---

## Current state

### CLI (`aetherd sir inject`)

```
aetherd sir inject <symbol_selector> --intent "<text>"
    --behavior "<text>"       # optional
    --edge-cases "<text>"     # optional
    --force                   # overwrite higher-quality existing SIR
    --dry-run                 # preview without persisting
    --no-embed                # skip re-embedding
```

Missing: `--side-effects`, `--dependencies`, `--error-modes`, `--complexity`, `--confidence`, `--inputs`, `--outputs`, `--model`.

### MCP (`aether_sir_inject`)

Same field limitations as CLI. The MCP request struct mirrors `SirInjectArgs`.

### Inject logic behavior (unchanged by this stage)

1. Find symbol by selector
2. Load existing SIR or create new
3. Update fields with provided values
4. Set `generation_pass = "injected"`, `generation_model = "manual"` ← **this changes**
5. Persist to SQLite, mirror to `.aether/sirs/` if configured
6. Synchronous re-embed if embeddings enabled
7. Update `prompt_hash`
8. Write `sir_fingerprint_history` row with `trigger = "inject"`

---

## What changes

### 1. CLI flags — expand `SirInjectArgs`

Add to `SirInjectArgs` in `crates/aetherd/src/cli.rs`:

```rust
/// Comma-separated side effects (e.g., "database write,audit log")
#[arg(long)]
pub side_effects: Option<String>,

/// Comma-separated dependencies (e.g., "sqlx::PgPool,chrono::Utc")
#[arg(long)]
pub dependencies: Option<String>,

/// Comma-separated error modes (e.g., "PaymentError::InsufficientFunds,sqlx::Error")
#[arg(long)]
pub error_modes: Option<String>,

/// Comma-separated input types (e.g., "amount: f64,account_id: String")
#[arg(long)]
pub inputs: Option<String>,

/// Comma-separated output types (e.g., "Result<(), PaymentError>")
#[arg(long)]
pub outputs: Option<String>,

/// Complexity level: Low, Medium, High, Critical
#[arg(long)]
pub complexity: Option<String>,

/// Confidence score 0.0-1.0 (default: 0.95 for agent-authored, 0.5 for manual)
#[arg(long)]
pub confidence: Option<f32>,

/// Model that authored this SIR (e.g., "claude-opus-4-6", "codex-mini-latest")
/// Defaults to "manual" if not specified
#[arg(long)]
pub model: Option<String>,
```

Parsing: split comma-separated strings into `Vec<String>`, trimming whitespace. Empty strings after trim are filtered out.

### 2. MCP request — expand `AetherSirInjectRequest`

Add matching optional fields:

```rust
pub struct AetherSirInjectRequest {
    pub symbol: String,
    pub file_path: Option<String>,          // helps disambiguate symbol selector
    pub intent: String,                      // required
    pub behavior: Option<String>,
    pub edge_cases: Option<String>,
    // NEW fields:
    pub side_effects: Option<Vec<String>>,
    pub dependencies: Option<Vec<String>>,
    pub error_modes: Option<Vec<String>>,
    pub inputs: Option<Vec<String>>,
    pub outputs: Option<Vec<String>>,
    pub complexity: Option<String>,
    pub confidence: Option<f32>,
    pub model: Option<String>,               // authoring model name
}
```

Note: MCP accepts `Vec<String>` directly (JSON arrays). CLI uses comma-separated strings parsed into `Vec<String>`.

### 3. MCP tool description update

Update the `#[tool(...)]` description for `aether_sir_inject` to:

```
"Inject or update a symbol's SIR with full annotation fields. Use after creating or modifying code to provide complete semantic intelligence — intent, side effects, dependencies, error modes, complexity, and confidence. The authoring model name is recorded for provenance tracking."
```

### 4. Inject logic — populate all SirAnnotation fields

In the inject execution path (wherever `SirAnnotation` is constructed from inject args):

```rust
let sir = SirAnnotation {
    intent: args.intent.clone(),
    inputs: args.inputs.clone().unwrap_or_default(),
    outputs: args.outputs.clone().unwrap_or_default(),
    side_effects: args.side_effects.clone().unwrap_or_else(|| existing_sir.side_effects.clone()),
    dependencies: args.dependencies.clone().unwrap_or_else(|| existing_sir.dependencies.clone()),
    error_modes: args.error_modes.clone().unwrap_or_else(|| existing_sir.error_modes.clone()),
    confidence: args.confidence.unwrap_or(0.95),
    method_dependencies: existing_sir.method_dependencies.clone(), // preserve if exists
};
```

Key behaviors:
- Fields provided by the agent **replace** existing values (not merge)
- Fields NOT provided **preserve** existing values from any prior SIR
- If no prior SIR exists, missing fields default to empty vecs / 0.95 confidence
- `confidence` defaults to **0.95** (not 0.5) when injected — agent-authored SIRs are high-confidence by definition
- `generation_model` uses `args.model.unwrap_or("manual".to_owned())` instead of hardcoded `"manual"`

### 5. Agent config template updates

In `crates/aetherd/src/templates/claude_md.rs` (and codex/cursor equivalents), add to the mandatory tier:

```
After creating or significantly modifying source files, call `aether_sir_inject`
for each key symbol with the full SIR fields:
- intent: what the symbol does (one sentence)
- side_effects: observable effects (database writes, network calls, file I/O, logging)
- dependencies: external crates/modules/APIs used
- error_modes: specific error types that can be returned/thrown
- complexity: Low | Medium | High | Critical
- confidence: 0.95
- model: your model identifier (e.g., "claude-opus-4-6")

You wrote the code — you know these fields better than any external inference model.
```

---

## Out of scope

- `method_dependencies` injection via CLI/MCP (complex HashMap structure — keep as inference-only for now)
- Bulk inject from JSONL (future enhancement)
- Auto-triggering inject via hooks (separate concern — hooks call the tool, this stage makes the tool accept all fields)
- Changing the `SirAnnotation` struct itself (already has all needed fields)
- Dashboard UI for viewing agent-authored vs inference-authored SIR (future Phase 9 enhancement)

---

## Decision

### #110. Full SIR inject with model provenance

**Date:** 2026-03-25
**Status:** Proposed

**Context:** Coding agents (Claude Code, Codex, Cursor) have complete semantic knowledge of code they write but can only inject a partial SIR (intent + behavior + edge-cases). The `generation_model` field is hardcoded to `"manual"`, losing provenance of which agent authored the SIR.

**Decision:** Expand `sir inject` (CLI + MCP) to accept all `SirAnnotation` fields and a `--model` flag. Default confidence for agent-authored SIRs is 0.95. `generation_model` records the actual authoring model. Agent config templates (`init-agent`) updated with guidance to use full inject.

**Rationale:** Zero-cost intelligence amplification. Every file a coding agent touches gets premium-quality SIR without additional inference API calls. Provenance tracking enables quality analysis (e.g., "Opus-authored SIRs have 40% richer side_effects than flash-lite").

---

## Cross-agent compatibility

| Agent | MCP Support | Hook Mechanism | Config File |
|-------|-------------|----------------|-------------|
| Claude Code | Native MCP plugin | PostToolUse / Stop hooks | CLAUDE.md + .claude/settings.json |
| Codex CLI | MCP via config | Codex hook system | .codex-instructions |
| Cursor | MCP via .cursor/mcp.json | afterFileEdit hook | .cursor/rules |
| Any HTTP agent | Decision #109 HTTP endpoint | N/A (call REST API directly) | N/A |
| Windsurf/Cline/etc | MCP if supported | Varies | Agent-specific config |

The MCP tool is the universal interface. Every agent that supports MCP can call `aether_sir_inject` with full fields. The hooks and config files just automate *when* the call happens.

---

## Pass criteria

1. `aetherd sir inject my_fn --intent "..." --side-effects "db write,log" --dependencies "sqlx,tokio" --error-modes "sqlx::Error" --confidence 0.95 --model "claude-opus-4-6"` persists a complete SIR with all fields populated.
2. `aether_sir_inject` MCP tool accepts `side_effects`, `dependencies`, `error_modes`, `inputs`, `outputs`, `complexity`, `confidence`, `model` as optional JSON fields.
3. Omitted fields preserve existing SIR values (not overwritten with empty).
4. `generation_model` in `sir_meta` reflects the `--model` flag value, not hardcoded `"manual"`.
5. Default confidence is 0.95 when `--confidence` not specified (agent-authored = high confidence).
6. `--dry-run` shows the complete SIR that would be persisted, including all new fields.
7. Re-embedding still works correctly with the richer SIR content.
8. Existing inject behavior (intent-only) is backward compatible — old callers still work.
9. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test -p aetherd`, `cargo test -p aether-mcp` pass.
