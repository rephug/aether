# Phase CC.6 — Prompt Enhancer

**Phase:** CC — Claude Code Integration
**Prerequisites:** CC.2b (aether_sir_context MCP tool — for context assembly patterns)
**Estimated Claude Code Runs:** 1
**Decision:** #108 — Template-first prompt enhancement with optional LLM rewrite

---

## Purpose

Add a prompt enhancement feature that takes a vague natural language prompt (e.g., "fix the auth bug"), queries AETHER's intelligence stores for relevant context, and produces a structured, context-rich prompt ready to paste into any AI coding tool.

Inspired by Augment Code's Prompt Enhancer, but differentiated by AETHER's deeper intelligence: SIR annotations, dependency graphs, coupling scores, health data, community structure, and drift warnings — not just file references.

---

## Why This Matters

Every AI coding session starts with context assembly. Today that means:
- Manually searching for relevant files
- Copy-pasting function signatures and comments
- Trying to remember which modules are coupled
- Hoping the agent figures out the rest

AETHER already has all this intelligence indexed. The Prompt Enhancer makes it the **first step** of every coding session — regardless of which tool you're using.

---

## Compatibility

| Surface | How it works |
|---------|-------------|
| **CLI** (`aether enhance "..."`) | Universal — works before any tool |
| **MCP** (`aether_enhance_prompt`) | Native in Claude Code, Gemini CLI, Cursor |
| **VS Code** (CC.7) | In-editor keybinding, replaces input in-place |
| **Codex CLI** | Call CLI via bash in prompt preamble |
| **ChatGPT / browser** | CLI → clipboard → paste |

---

## Decision #108: Template-first enhancement

**Context:** Two approaches to prompt rewriting: (A) LLM rewrites the prompt using AETHER context, (B) template engine assembles context deterministically.

**Decision:** Template-first with optional LLM rewrite mode.

**Rationale:**
- AETHER's context is already so structured that a good template produces better output than a smart LLM with shallow context
- Templates are faster (no API call for assembly), cheaper ($0), and deterministic
- LLM rewrite mode available via `--rewrite` flag for users who want natural prose
- LLM rewrite uses the existing inference pipeline (Gemini Flash default, configurable)

---

## Architecture

### Two-Phase Pipeline

**Phase 1 — Intent Extraction (lightweight LLM call)**

Takes the user's raw prompt and extracts structured targets:

```rust
pub struct EnhanceIntent {
    /// Original prompt text
    pub raw_prompt: String,
    /// Extracted target symbols (function names, struct names, etc.)
    pub target_symbols: Vec<String>,
    /// Extracted target files or paths
    pub target_files: Vec<String>,
    /// Extracted concepts/topics (e.g., "authentication", "rate limiting")
    pub concepts: Vec<String>,
    /// Detected task type
    pub task_type: TaskType,
}

pub enum TaskType {
    BugFix,
    Refactor,
    NewFeature,
    Test,
    Documentation,
    Investigation,
    General,
}
```

This call uses the triage model (flash-lite by default). Prompt:

```
Given this coding task prompt, extract:
1. Any specific symbol names (functions, structs, modules) mentioned or implied
2. Any file paths mentioned or implied
3. Key concepts or domains referenced
4. The task type (bug_fix, refactor, new_feature, test, documentation, investigation, general)

Respond in JSON only. No explanation.

Prompt: "{raw_prompt}"
```

**Fallback:** If the LLM call fails or is unavailable (no API key, offline), fall back to keyword extraction: split on whitespace, match against indexed symbol names and file paths via the store's search methods. This ensures the feature works offline with local-only AETHER.

**Phase 2 — Context Assembly + Template Rendering**

Using the extracted targets, query AETHER stores:

1. **Symbol resolution:** For each `target_symbol`, run `store.search_symbols()` to find matches. Take top 3 by relevance.
2. **File resolution:** For each `target_file`, run `store.list_symbols_for_file()`.
3. **Concept search:** For each `concept`, run hybrid search (lexical + semantic if embeddings available).
4. **For each resolved symbol, gather:**
   - SIR annotation (intent, inputs, outputs, side_effects, error_modes)
   - Direct dependencies and callers (1 level)
   - Health score + any active warnings
   - Generation pass level (scan/triage/deep)
   - Drift status if flagged
5. **For the workspace, gather:**
   - Community structure around target symbols
   - Coupling data between target files
   - Active contracts on target symbols

Token budget: configurable, default 8000 tokens. Uses the same proportional allocation as `sir_context`:
- Source context: 35%
- SIR annotations: 25%
- Graph neighbors: 20%
- Health/drift warnings: 10%
- Coupling/community: 10%

### Template Output

The default template produces structured markdown:

```markdown
## Enhanced Prompt

{original_prompt}

## Relevant Context

### Target Symbols

**`{symbol.qualified_name}`** ({symbol.kind} in `{symbol.file_path}`)
- **Intent:** {sir.intent}
- **Inputs:** {sir.inputs}
- **Side effects:** {sir.side_effects}
- **Error modes:** {sir.error_modes}
- **Health:** {health_score}/100 {health_warnings}
- **Dependencies:** {dep_list}
- **Callers:** {caller_list}

### Related Files
{file_list_with_symbol_counts}

### Architectural Notes
{coupling_warnings}
{drift_warnings}
{community_context}

### Conventions
{coding_patterns_if_available}
```

### Optional LLM Rewrite Mode (`--rewrite`)

When `--rewrite` is passed, the template output is sent to a second LLM call (Gemini Flash default) with this system prompt:

```
You are a prompt engineering assistant for a software development AI agent.
You have been given a developer's original coding prompt and rich context from
a codebase intelligence engine (AETHER).

Rewrite the original prompt into a clear, detailed, actionable prompt that:
1. States the specific goal clearly
2. References relevant files and symbols by name
3. Mentions architectural constraints (coupling, health issues, drift)
4. Suggests a logical approach based on the dependency graph
5. Warns about edge cases from the SIR error modes

Keep the rewritten prompt concise but thorough. Do not include the raw context
dump — synthesize it into natural instructions.
```

---

## New Files

```
crates/aether-mcp/src/tools/enhance.rs    # MCP tool: aether_enhance_prompt
crates/aetherd/src/enhance.rs             # Core enhancement logic (shared)
crates/aetherd/src/enhance_templates.rs   # Template rendering
```

---

## CLI Interface

```bash
# Basic — template mode, output to stdout
aether enhance "fix the auth bug in the login flow"

# Copy to clipboard (requires xclip/pbcopy)
aether enhance "fix the auth bug" --clipboard

# LLM rewrite mode
aether enhance "fix the auth bug" --rewrite

# JSON output (for piping to other tools)
aether enhance "fix the auth bug" --output json

# Custom token budget
aether enhance "fix the auth bug" --budget 16000

# Skip LLM intent extraction, use keyword matching only
aether enhance "fix the auth bug" --offline
```

### CLI Args (in `cli.rs`)

```rust
/// Enhance a prompt with AETHER codebase intelligence
Enhance {
    /// The prompt to enhance
    #[arg(required = true)]
    prompt: String,

    /// Copy enhanced prompt to clipboard
    #[arg(long)]
    clipboard: bool,

    /// Use LLM to rewrite the enhanced prompt into natural prose
    #[arg(long)]
    rewrite: bool,

    /// Output format: text (default) or json
    #[arg(long, default_value = "text")]
    output: String,

    /// Token budget for context assembly (default: 8000)
    #[arg(long, default_value_t = 8000)]
    budget: usize,

    /// Skip LLM intent extraction, use keyword matching only
    #[arg(long)]
    offline: bool,
},
```

---

## MCP Tool

```rust
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AetherEnhancePromptRequest {
    /// The raw prompt to enhance
    pub prompt: String,

    /// Token budget for context (default: 8000)
    #[serde(default = "default_budget")]
    pub budget: usize,

    /// Whether to use LLM rewrite mode (default: false)
    #[serde(default)]
    pub rewrite: bool,

    /// Output format: "text" or "json" (default: "text")
    #[serde(default = "default_text")]
    pub format: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AetherEnhancePromptResponse {
    /// The enhanced prompt text
    pub enhanced_prompt: String,

    /// Symbols that were resolved and included
    pub resolved_symbols: Vec<String>,

    /// Files referenced in the enhanced prompt
    pub referenced_files: Vec<String>,

    /// Whether LLM rewrite was used
    pub rewrite_used: bool,

    /// Token count of the enhanced prompt
    pub token_count: usize,

    /// Warnings (e.g., "no symbols matched", "embeddings unavailable")
    pub warnings: Vec<String>,
}
```

---

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| No symbols match the prompt | Return prompt with workspace-level context only (file tree, top-level health) |
| No API key configured | Fall back to keyword extraction (no LLM intent extraction) |
| Prompt is already detailed | Extract targets from explicit mentions, skip LLM extraction |
| Empty workspace (not indexed) | Return original prompt with warning: "Workspace not indexed. Run `aether index` first." |
| `--rewrite` but LLM fails | Return template output with warning that rewrite failed |
| Clipboard tool not available | Print to stdout with message: "Install xclip or pbcopy for --clipboard" |
| Very long prompt (>500 words) | Truncate to first 500 words for intent extraction, pass full prompt through to output |

---

## Config Additions

```toml
[enhance]
# Default token budget for context assembly
budget = 8000
# Default model for intent extraction (uses triage provider)
# Rewrite mode uses the deep provider
# Both fall back to keyword extraction if unavailable
```

No new config section is strictly needed — reuses existing `[inference]` provider config. The `[enhance]` section is optional for budget override.

---

## Pass Criteria

1. `aether enhance "fix the auth bug"` produces structured context output with resolved symbols
2. `--offline` mode works without any LLM API key
3. `--rewrite` mode produces natural prose (when API key available)
4. `--clipboard` copies to clipboard (or prints helpful error)
5. `--output json` returns parseable JSON with all response fields
6. MCP tool `aether_enhance_prompt` returns structured response
7. Empty workspace returns original prompt with warning
8. Token budget is respected (output doesn't exceed budget)
9. `cargo fmt --all --check` passes
10. `cargo clippy -p aetherd -p aether-mcp -- -D warnings` passes
11. `cargo test -p aetherd` and `cargo test -p aether-mcp` pass

---

## Commit

**PR title:** `feat: aether enhance — prompt enhancer with codebase intelligence context`

**PR body:**
```
Stage CC.6 of the Claude Code Integration phase.

Adds prompt enhancement that takes a vague coding prompt and enriches it
with AETHER's codebase intelligence:

- Intent extraction via triage LLM (with offline keyword fallback)
- Context assembly: SIR annotations, dependencies, callers, health scores,
  drift warnings, coupling data, community structure
- Template-based output (default) with optional LLM rewrite mode
- CLI: `aether enhance "prompt" [--rewrite] [--clipboard] [--output json]`
- MCP: `aether_enhance_prompt` tool for agent integration
- Configurable token budget (default 8000)

Inspired by Augment Code's Prompt Enhancer, differentiated by AETHER's
semantic intelligence depth (SIR, graph, coupling, health, drift).
```
