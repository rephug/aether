# Codex Prompt — Full SIR Inject: Agent-Authored Intelligence

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Do NOT run `cargo test --workspace` — OOM risk. Always per-crate.

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -B feature/full-sir-inject /home/rephu/feature/full-sir-inject
cd /home/rephu/feature/full-sir-inject
```

## GOAL

Expand `sir inject` (both CLI and MCP tool) to accept the **complete SirAnnotation fields**: `side_effects`, `dependencies`, `error_modes`, `inputs`, `outputs`, `complexity`, `confidence`, plus a `--model` flag to record which AI model authored the SIR. Currently only `--intent`, `--behavior`, and `--edge-cases` are supported.

This enables coding agents (Claude Code, Codex, Cursor) to inject full-quality SIR at zero inference cost — the agent wrote the code and knows all these fields already.

## SOURCE INSPECTION (MANDATORY — do this BEFORE writing any code)

Read and verify these files. If any assumption below is wrong, adapt your implementation to reality.

1. **`crates/aetherd/src/cli.rs`** — Find `SirInjectArgs` struct. Verify it has:
   - `symbol` (positional arg or named)
   - `intent` (required string)
   - `behavior` (optional string)
   - `edge_cases` (optional string)
   - `force` (bool flag)
   - `dry_run` (bool flag)
   - `no_embed` (bool flag)
   
   Note the EXACT field names, types, and `#[arg(...)]` attributes used. Your new fields must match the same patterns.

2. **`crates/aether-sir/src/lib.rs`** — Find `SirAnnotation` struct. Verify it has these fields:
   - `intent: String`
   - `inputs: Vec<String>`
   - `outputs: Vec<String>`
   - `side_effects: Vec<String>`
   - `dependencies: Vec<String>`
   - `error_modes: Vec<String>`
   - `confidence: f32`
   - `method_dependencies: Option<HashMap<String, Vec<String>>>` (added in Phase 8.20)
   
   Note the exact types. These are the fields we need to be able to inject.

3. **`crates/aetherd/src/sir_inject.rs`** (or wherever the inject execution logic lives — it might be in `sir_pipeline.rs` or a dedicated module). Find where:
   - The `SirAnnotation` is constructed from inject args
   - `generation_pass` is set to `"injected"`
   - `generation_model` is set to `"manual"` ← we change this
   - The SIR is persisted to SQLite
   - Re-embedding happens
   - Fingerprint history is written
   
   Note the EXACT function signature and how existing fields are mapped.

4. **`crates/aether-mcp/src/lib.rs`** (or `tools/sir_inject.rs` or wherever the MCP inject tool lives) — Find:
   - `AetherSirInjectRequest` struct definition
   - The `#[tool(...)]` attribute with name `"aether_sir_inject"`
   - How the MCP request maps to the inject logic
   
   Note: the MCP tool might be named `aether_sir_inject` or similar. Search for "inject" in the MCP crate.

5. **`crates/aetherd/src/templates/claude_md.rs`** — Find where the CLAUDE.md template content is defined. We will add guidance about using full SIR inject fields.

6. **Check if `aether_sir_inject` is an MCP tool or only a CLI command.** Search:
   ```bash
   grep -rn "sir_inject\|sir-inject\|SirInject" crates/aether-mcp/src/
   ```
   If the MCP tool exists, update its request struct. If it does NOT exist as an MCP tool yet (only CLI), note this — the MCP tool was specified in Phase 10.3 but may or may not be implemented yet.

## IMPLEMENTATION

After source inspection, implement these changes:

### Change 1: Expand CLI args

In `SirInjectArgs` (wherever you found it in step 1), add these new optional fields AFTER the existing `edge_cases` field and BEFORE the flag fields (`force`, `dry_run`, `no_embed`):

```rust
    /// Comma-separated side effects (e.g., "database write,audit log,network call")
    #[arg(long)]
    pub side_effects: Option<String>,

    /// Comma-separated dependencies (e.g., "sqlx::PgPool,chrono::Utc,tokio::fs")
    #[arg(long)]  
    pub dependencies: Option<String>,

    /// Comma-separated error modes (e.g., "PaymentError::InsufficientFunds,sqlx::Error")
    #[arg(long)]
    pub error_modes: Option<String>,

    /// Comma-separated input type descriptions (e.g., "amount: f64,account_id: String")
    #[arg(long)]
    pub inputs: Option<String>,

    /// Comma-separated output type descriptions (e.g., "Result<(), PaymentError>")
    #[arg(long)]
    pub outputs: Option<String>,

    /// Complexity: Low, Medium, High, Critical
    #[arg(long)]
    pub complexity: Option<String>,

    /// Confidence score 0.0-1.0 (default: 0.95 for agent-authored SIRs)
    #[arg(long)]
    pub confidence: Option<f32>,

    /// Model that authored this SIR (e.g., "claude-opus-4-6"). Defaults to "manual".
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,
```

Add a helper function near the inject logic (or in a utils module):

```rust
fn parse_comma_list(input: &Option<String>) -> Vec<String> {
    input
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|item| item.trim().to_owned())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}
```

### Change 2: Expand MCP request struct (if MCP tool exists)

If `AetherSirInjectRequest` exists, add optional fields:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_effects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_modes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
```

All new fields are `Option` with `serde(default)` — backward compatible. Existing callers that only send `intent` still work.

Update the `#[tool(...)]` description to mention the new fields:
```
"Inject or update a symbol's complete SIR annotation. Accepts all fields: intent, side_effects, dependencies, error_modes, inputs, outputs, complexity, confidence. Use --model to record which AI model authored the SIR for provenance tracking."
```

### Change 3: Update inject execution logic

Find where the `SirAnnotation` is built from inject args. Change the construction to:

```rust
// Load existing SIR if any (for field preservation)
let existing_sir = store.read_sir_blob(&symbol_id)
    .ok()
    .flatten()
    .and_then(|blob| serde_json::from_str::<SirAnnotation>(&blob).ok());

let empty_sir = SirAnnotation::default(); // or construct with empty fields
let base = existing_sir.as_ref().unwrap_or(&empty_sir);

let sir = SirAnnotation {
    intent: args.intent.clone(),
    inputs: if args_has_inputs { parse_inputs(args) } else { base.inputs.clone() },
    outputs: if args_has_outputs { parse_outputs(args) } else { base.outputs.clone() },
    side_effects: if args_has_side_effects { parse_side_effects(args) } else { base.side_effects.clone() },
    dependencies: if args_has_dependencies { parse_dependencies(args) } else { base.dependencies.clone() },
    error_modes: if args_has_error_modes { parse_error_modes(args) } else { base.error_modes.clone() },
    confidence: args.confidence.unwrap_or(0.95),
    method_dependencies: base.method_dependencies.clone(),
};
```

**CRITICAL:** The exact field names and how args are accessed will depend on what you found in source inspection. The pattern is:
- If the caller provided a field → use the caller's value
- If the caller did NOT provide a field → preserve the existing SIR value
- If no existing SIR → use empty vec / 0.95 confidence

For the CLI path, use `parse_comma_list()` to convert comma-separated strings to `Vec<String>`.
For the MCP path, the `Vec<String>` comes directly from JSON deserialization.

### Change 4: Update generation_model

Find where `generation_model` is set to `"manual"`. Change to:

```rust
let generation_model = args.model
    .as_deref()
    .unwrap_or("manual")
    .to_owned();
```

This records the actual authoring model (e.g., `"claude-opus-4-6"`) in `sir_meta`, enabling provenance queries like "show me all SIRs authored by Opus vs flash-lite."

### Change 5: Update agent config templates

In `crates/aetherd/src/templates/claude_md.rs`, find the mandatory actions section and add or update:

```
After creating or significantly modifying source files, call `aether_sir_inject`
for each key symbol with the full SIR fields you know:
- intent: what the symbol does (one sentence)
- side_effects: observable effects (database writes, network calls, file I/O, logging)  
- dependencies: external crates/modules/APIs the symbol uses
- error_modes: specific error types returned or thrown
- complexity: Low | Medium | High | Critical
- confidence: 0.95
- model: your model identifier

You wrote the code — you know these fields better than any inference model will.
```

Apply the same update to `codex_instructions.rs` and `cursor_rules.rs` if they exist and have equivalent mandatory action sections.

### Change 6: Bump AETHER_AGENT_SCHEMA_VERSION

In `crates/aether-core/src/lib.rs` (or wherever `AETHER_AGENT_SCHEMA_VERSION` is defined), increment the version number by 1. This signals to users that their generated agent configs are stale and should be regenerated via `init-agent --force`.

## SCOPE GUARD

Only modify files in:
- `crates/aetherd/src/` (CLI args + inject logic)
- `crates/aether-mcp/src/` (MCP request struct + tool description)
- `crates/aetherd/src/templates/` (agent config templates)
- `crates/aether-core/src/` (schema version bump only)

Do NOT modify:
- `crates/aether-sir/` (SirAnnotation already has all fields)
- `crates/aether-store/` (no schema changes needed)
- `crates/aether-infer/` (no prompt changes)
- Any migration files (no SQLite schema change)

## VALIDATION

```bash
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo test -p aetherd
cargo test -p aether-mcp
cargo test -p aether-core
```

All must pass. Do NOT run `cargo test --workspace`.

## FUNCTIONAL VERIFICATION

After implementation, verify these scenarios work:

1. **Full inject via CLI:**
```bash
cd /home/rephu/feature/full-sir-inject
cargo run -p aetherd -- --workspace /tmp/test-workspace sir inject test_fn \
    --intent "Validates payment amounts against account balance" \
    --side-effects "database read,audit log write" \
    --dependencies "sqlx::PgPool,chrono::Utc" \
    --error-modes "PaymentError::InsufficientFunds,PaymentError::InvalidAmount" \
    --complexity "Medium" \
    --confidence 0.95 \
    --model "claude-opus-4-6" \
    --dry-run
```
Should show the complete SIR that would be persisted with all fields populated.

2. **Intent-only inject still works (backward compat):**
```bash
cargo run -p aetherd -- --workspace /tmp/test-workspace sir inject test_fn \
    --intent "Simple test function" \
    --dry-run
```
Should work without error, using defaults for omitted fields.

## COMMIT

```bash
git add -A
git commit -m "feat: expand sir inject to accept full SIR annotation fields

- Add CLI flags: --side-effects, --dependencies, --error-modes,
  --inputs, --outputs, --complexity, --confidence, --model
- Add matching optional fields to MCP AetherSirInjectRequest
- Provided fields replace existing; omitted fields preserve prior SIR
- generation_model records actual authoring model instead of 'manual'
- Default confidence 0.95 for agent-authored SIRs (was 0.5)
- Update agent config templates with full inject guidance
- Bump AETHER_AGENT_SCHEMA_VERSION

Decision #110: Full SIR inject with model provenance"
```

## POST-IMPLEMENTATION

```bash
git push origin feature/full-sir-inject
```

Create PR via GitHub web UI:
- **Title:** feat: expand sir inject to accept full SIR annotation fields
- **Body:**
  ```
  ## Summary
  Upgrades `sir inject` (CLI + MCP) to accept the complete SirAnnotation schema.
  Coding agents (Claude Code, Codex, Cursor) can now inject full-quality SIR
  including side_effects, dependencies, error_modes, complexity, and confidence
  at zero additional inference cost.
  
  ## Changes
  - CLI: 8 new optional flags on `sir inject`
  - MCP: 8 new optional fields on `aether_sir_inject` request
  - Inject logic: provided fields replace, omitted fields preserve existing
  - generation_model: records actual model name (e.g., "claude-opus-4-6")
  - Default confidence: 0.95 for agent-authored SIR (was 0.5)
  - Agent templates: updated with full inject guidance
  - AETHER_AGENT_SCHEMA_VERSION bumped
  
  ## Decision
  #110 — Full SIR inject with model provenance
  ```

After merge:
```bash
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/full-sir-inject
git branch -D feature/full-sir-inject
```
