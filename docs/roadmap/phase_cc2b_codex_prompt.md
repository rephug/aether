# Claude Code Prompt — Stage CC.2b: aether_sir_context MCP Tool

## Preamble

```bash
# Preflight
cd /home/rephu/projects/aether
git status --porcelain        # Must be clean
git pull --ff-only

# Build environment
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=2
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR

# Branch + worktree
git worktree add -B feature/cc2b-sir-context-mcp /home/rephu/feature/cc2b-sir-context-mcp
cd /home/rephu/feature/cc2b-sir-context-mcp
```

---

## Context

The CLI command `aetherd sir-context` assembles a multi-layer, token-budgeted
context document for a symbol — including source code, SIR annotation, graph
neighbors, tests, coupling, memory, health, and drift signals. It's 4018 lines
in `aetherd/src/sir_context.rs` and all its internal functions are `pub(crate)`.

This stage exposes the equivalent capability as an MCP tool. Claude Code currently
has to make 3-5 separate MCP calls (`aether_get_sir` + `aether_dependencies` +
read source file + `aether_health`) to assemble context for one symbol. The
`aether_sir_context` tool does it in one call with automatic token budgeting.

**Important design constraint:** The MCP binary (`aether-mcp`) cannot call
`aetherd` functions — they're in different binary crates. This tool reimplements
the context assembly using store methods and library crate functions that are
available to `aether-mcp`. It does NOT need to replicate all 9 layers or the
full complexity of the CLI tool. A practical subset covering the most valuable
layers is sufficient.

---

## Source Inspection (MANDATORY — do this before writing any code)

1. Read `crates/aetherd/src/sir_context.rs` lines 1-60 — understand the layer
   structure (`LAYER_SUGGESTIONS`), `ContextTarget`, `ContextFormat`, and
   the overall architecture.

2. Read the `prepare_symbol_target` function in `sir_context.rs` — this is the
   core function that assembles context for a single symbol. Note which store
   methods it calls and what data it collects.

3. Read `crates/aether-mcp/src/tools/refactor.rs` lines 580-650 — there's
   already a `build_sir_context` function that does a simpler version of context
   assembly for refactor-prep. Understand what it does and reuse the pattern.

4. Check what store methods are available from `aether-mcp`:
   ```bash
   grep "SymbolCatalogStore\|SirStateStore\|SymbolRelationStore\|TestIntentStore\|DriftStore\|ProjectNoteStore" crates/aether-mcp/src/tools/*.rs | head -20
   ```

5. Check how source files are read in the MCP crate:
   ```bash
   grep -rn "read_to_string\|fs::read\|source_code\|source_text" crates/aether-mcp/src/ | head -15
   ```

6. Check how the workspace path is accessible:
   ```bash
   grep "workspace\|root_path\|store_path" crates/aether-mcp/src/state.rs | head -15
   ```

7. Read `crates/aether-store/src/lib.rs` — find `SymbolRelationStore` trait methods:
   ```bash
   grep "fn.*callers\|fn.*dependencies\|fn.*neighbors\|fn.*relations" crates/aether-store/src/lib.rs | head -10
   ```

---

## Implementation

### Step 1: Request/Response types

Create `crates/aether-mcp/src/tools/context.rs` with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirContextRequest {
    /// Symbol ID or qualified name selector
    pub symbol: String,
    /// Maximum token budget (default 8000)
    pub max_tokens: Option<usize>,
    /// Output format: "markdown" (default) or "json"
    pub format: Option<String>,
    /// Layers to include. Default: all available.
    /// Valid layers: "source", "sir", "graph", "tests", "health", "reasoning"
    pub include_layers: Option<Vec<String>>,
    /// Dependency traversal depth (1-3, default 1)
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SirContextLayer {
    /// Layer name (e.g., "source", "sir", "graph")
    pub name: String,
    /// Layer content
    pub content: String,
    /// Approximate token count for this layer
    pub token_estimate: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AetherSirContextResponse {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    /// Assembled context document (markdown or JSON depending on format)
    pub context: String,
    /// Individual layers with their token estimates
    pub layers: Vec<SirContextLayer>,
    /// Total estimated tokens used
    pub total_tokens: usize,
    /// Token budget remaining
    pub budget_remaining: usize,
}
```

### Step 2: Implement the logic

Add `aether_sir_context_logic` method on `AetherMcpServer`.

**Layer assembly (simplified from CLI's 9 layers to 6 practical layers):**

1. **Source layer** — Read the symbol's source file from disk using the workspace
   path. Extract the symbol's span (start_line..end_line from the symbols table)
   or include the full file if span is unavailable. Apply token budget trimming —
   if source exceeds ~40% of budget, truncate from the middle with a
   `[... truncated ...]` marker.

2. **SIR layer** — Call `store.read_sir_blob(symbol_id)` and parse the SIR
   annotation. Format as readable text: intent, side_effects, error_modes,
   confidence.

3. **Graph layer** — Get direct callers and callees using `SymbolRelationStore`
   methods. For each neighbor (up to 10), include: qualified_name, file_path,
   edge type (calls/called_by/implements), and a one-line intent summary from
   their SIR (if available). If `depth > 1`, traverse one more level (but cap
   at 20 total neighbors to stay within budget).

4. **Health layer** — Get health signals for the symbol: risk_score, pagerank,
   betweenness, in_cycle, test_count. Use the same health data access pattern
   as `aether_health_logic`.

5. **Reasoning layer** — If `reasoning_trace` is available in the SIR metadata,
   include it. This is the triage model's thinking about the symbol.

6. **Tests layer** — If `TestIntentStore` methods are available, query test
   intents related to this symbol's file. Include up to 5 test intent summaries.

**Token budgeting approach:**

Use `content.len() / 4` as a rough token estimate (matching the CLI's
`CHARS_PER_TOKEN` of 3.5, rounded). Allocate budget proportionally:
- Source: 40% of budget
- SIR: 15%
- Graph: 20%
- Health: 5%
- Reasoning: 10%
- Tests: 10%

Assemble layers in order. If a layer exceeds its allocation, truncate it.
Skip layers the user excluded via `include_layers`. If total is under budget,
don't truncate anything.

**Markdown format (default):**

```markdown
# Context: SqliteStore::reconcile_and_prune

## Source
```rust
pub fn reconcile_and_prune(&self, ...) {
    // ...
}
```

## SIR Annotation
**Intent:** Removes orphaned symbol records and their associated data...
**Side Effects:** Deletes sir, sir_history, sir_quality rows for pruned symbols
**Error Modes:** ...
**Confidence:** 0.72
**Generation Pass:** triage

## Dependencies
### Callers (3)
- `handler_function` (aetherd/src/main.rs) — dispatches reconciliation on index completion
- ...

### Callees (5)
- `delete_symbol_records_for_ids` (aether-store/src/lifecycle.rs) — bulk delete by ID list
- ...

## Health
- Risk Score: 0.91
- PageRank: 0.0034
- Betweenness: 0.12
- In Cycle: no
- Test Count: 0

## Reasoning Trace
Model expressed uncertainty about transaction rollback behavior...

## Test Intents
- test_reconcile_removes_orphaned_sir: verifies sir rows are deleted for pruned symbols
- ...
```

**JSON format:**

Return the same data as a structured JSON object with each layer as a field.

### Step 3: Register in router.rs

Add to the `#[tool_router]` impl block:

```rust
#[tool(
    name = "aether_sir_context",
    description = "Assemble token-budgeted context for a symbol including source, SIR, graph neighbors, health, reasoning trace, and test intents in one call"
)]
pub async fn aether_sir_context(
    &self,
    Parameters(request): Parameters<AetherSirContextRequest>,
) -> Result<Json<AetherSirContextResponse>, McpError> {
    self.verbose_log("MCP tool called: aether_sir_context");
    let server = self.clone();
    tokio::task::spawn_blocking(move || server.aether_sir_context_logic(request))
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
}
```

### Step 4: Export types and register module

In `crates/aether-mcp/src/tools/mod.rs`:
1. Add `pub mod context;`
2. Add re-exports for request/response types

### Step 5: Tests

In `crates/aether-mcp/src/tools/context.rs`:
1. Test basic context assembly — symbol with SIR and source
2. Test token budget trimming — large source gets truncated
3. Test `include_layers` filtering — only requested layers included
4. Test JSON vs markdown format output
5. Test symbol not found error handling

---

## Scope guard

**New file: `crates/aether-mcp/src/tools/context.rs`.**
**Modified: `crates/aether-mcp/src/tools/mod.rs`, `crates/aether-mcp/src/tools/router.rs`.**

Do NOT modify store schema, aetherd, or any other crate. This tool reads existing
data only — it does not write.

---

## Validation gate

```bash
cargo fmt --all --check
cargo clippy -p aether-mcp -- -D warnings
cargo test -p aether-mcp
```

Do NOT run `cargo test --workspace` — OOM risk.

All commands must pass before committing.

---

## Commit

```bash
git add -A
git commit -m "feat(mcp): aether_sir_context — token-budgeted multi-layer context assembly via MCP"
```

**PR title:** `feat(mcp): aether_sir_context — token-budgeted multi-layer context assembly via MCP`

**PR body:**
```
Stage CC.2b of the Claude Code Audit Integration phase.

Adds aether_sir_context MCP tool that assembles token-budgeted context for
a symbol in one call, replacing 3-5 separate tool calls:

Layers: source code, SIR annotation, graph neighbors (callers + callees),
health signals, reasoning trace, and test intents.

Supports markdown (default) and JSON output formats.
Configurable token budget (default 8000) with proportional layer allocation.
Configurable layer selection and dependency depth (1-3).

Simplified reimplementation of the CLI sir-context command (4018 lines)
using store methods accessible from the MCP crate. Covers 6 of the CLI's
9 layers — omits coupling, memory, and broader_graph layers which can be
queried separately via aether_blast_radius, aether_recall, and
aether_call_chain if needed.
```

---

## Post-commit

```bash
git push origin feature/cc2b-sir-context-mcp
# Create PR via GitHub web UI with title + body above
# After merge:
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/cc2b-sir-context-mcp
git branch -D feature/cc2b-sir-context-mcp
```
