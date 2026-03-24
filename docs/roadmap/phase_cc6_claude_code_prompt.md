# Claude Code Prompt — CC.6: Prompt Enhancer Core + CLI + MCP

CRITICAL BUILD SETTINGS — use these for ALL cargo commands:
```
export CARGO_TARGET_DIR=/home/rephu/aether-target
export CARGO_BUILD_JOBS=16
export PROTOC=$(which protoc)
export RUSTC_WRAPPER=sccache
export TMPDIR=/home/rephu/aether-target/tmp
mkdir -p $TMPDIR
```

Read the spec first:
- `docs/roadmap/phase_cc6_prompt_enhancer.md`

Then read these source files in order:

**Context assembly patterns (follow these):**
- `crates/aether-mcp/src/tools/context.rs` (aether_sir_context — token-budgeted assembly pattern)
- `crates/aether-mcp/src/tools/router.rs` (MCP tool registration pattern)
- `crates/aether-mcp/src/tools/mod.rs` (module declarations)

**Store query methods (you call these):**
- `crates/aether-store/src/symbols.rs` (search_symbols, list_symbols_for_file, get_symbol)
- `crates/aether-store/src/sir_meta.rs` (get_sir_meta, read_sir_blob)
- `crates/aether-store/src/graph.rs` (store_get_callers, store_get_dependencies)

**Health/analysis (optional context layers):**
- `crates/aether-analysis/src/health.rs` (HealthAnalyzer — for health scores)
- `crates/aether-analysis/src/drift.rs` (DriftAnalyzer — for drift warnings)
- `crates/aether-analysis/src/coupling.rs` (CouplingAnalyzer — for coupling data)

**CLI patterns (follow these):**
- `crates/aetherd/src/cli.rs` (clap subcommand pattern)
- `crates/aetherd/src/main.rs` (command dispatch pattern)
- `crates/aetherd/src/sir_context.rs` (existing context assembly — reference, don't copy)

**Inference (for intent extraction LLM call):**
- `crates/aether-infer/src/lib.rs` (InferenceProvider trait, how to make LLM calls)
- `crates/aether-config/src/lib.rs` (config structure for inference providers)

## PREFLIGHT

```bash
cd /home/rephu/projects/aether
git status --porcelain
# Must be clean. If dirty, STOP and report.
git pull --ff-only
```

Create branch and worktree:
```bash
git worktree add -B feature/cc6-prompt-enhancer /home/rephu/feature/cc6-prompt-enhancer
cd /home/rephu/feature/cc6-prompt-enhancer
```

## SOURCE INSPECTION

Before writing code, verify these assumptions. If any is false, STOP and report:

1. `aether_sir_context` tool in `tools/context.rs` exists and demonstrates the
   pattern for token-budgeted context assembly using store queries.

2. The `AetherMcpServer` in `lib.rs` uses `#[tool(...)]` attribute macro for
   tool registration. Each tool method takes `Parameters<RequestType>` and
   returns `Result<Json<ResponseType>, McpError>`.

3. `store.search_symbols(query, limit)` or equivalent method exists for
   fuzzy symbol name matching. Check the exact signature.

4. `store.read_sir_blob(symbol_id)` returns `Option<String>` with the SIR JSON.
   `store.get_sir_meta(symbol_id)` returns `Option<SirMetaRecord>`.

5. `store.store_get_callers(qualified_name)` and `store.store_get_dependencies(qualified_name)`
   return `Vec<SymbolEdge>`. Check exact method names — they may be on a sub-trait.

6. The CLI in `cli.rs` uses clap derive with `#[command(subcommand)]` on an
   enum `Commands`. Check how existing subcommands like `SirContext` are defined.

7. Check whether `aether-infer` exposes a simple "send prompt, get text response"
   method or if you need to go through the full SIR pipeline. The intent extraction
   call just needs raw text completion, not structured SIR output.

8. Check if clipboard access crates (`arboard` or `cli-clipboard`) are already
   in the workspace `Cargo.toml`. If not, add `arboard` to `aetherd/Cargo.toml`.

## IMPLEMENTATION

### Step 1: Core enhancement logic (`crates/aetherd/src/enhance.rs`)

Create the core enhancement module with these components:

**`EnhanceIntent` struct** — extracted targets from the raw prompt:
- `target_symbols: Vec<String>` — symbol names found/implied
- `target_files: Vec<String>` — file paths found/implied  
- `concepts: Vec<String>` — domain concepts
- `task_type: TaskType` — enum: BugFix, Refactor, NewFeature, Test, Documentation, Investigation, General

**`EnhanceResult` struct** — the output:
- `enhanced_prompt: String`
- `resolved_symbols: Vec<String>`
- `referenced_files: Vec<String>`
- `rewrite_used: bool`
- `token_count: usize`
- `warnings: Vec<String>`

**`extract_intent_via_llm()` function:**
- Takes raw prompt string + inference provider
- Sends a structured prompt asking the LLM to extract symbols, files, concepts, task type
- Parses JSON response into `EnhanceIntent`
- On failure, falls back to `extract_intent_via_keywords()`

**`extract_intent_via_keywords()` function (offline fallback):**
- Split prompt on whitespace and punctuation
- Match tokens against `store.search_symbols(token, 5)` for each non-stop-word
- Match tokens against known file paths (list files from store)
- Classify task type by keyword presence ("fix"/"bug" → BugFix, "refactor" → Refactor, etc.)

**`assemble_context()` function:**
- Takes `EnhanceIntent` + store + token budget
- For each target symbol: resolve via store, gather SIR + deps + callers + health
- For each target file: list symbols, gather top-level SIR summaries
- For each concept: run search, take top 3 matches
- Apply token budget proportionally (same approach as sir_context)
- Return assembled context sections as structured data

**`render_template()` function:**
- Takes original prompt + assembled context
- Renders into structured markdown using the template from the spec
- Returns the final enhanced prompt string

**`enhance_prompt_core()` — the main entry point:**
- Orchestrates: extract intent → assemble context → render template
- If `rewrite` is true, sends template output to LLM for natural prose rewrite
- Returns `EnhanceResult`

### Step 2: Template rendering (`crates/aetherd/src/enhance_templates.rs`)

Separate file for the template strings and rendering logic. The template should
produce clean, readable markdown that any AI tool can consume. See the spec for
the template format.

### Step 3: CLI command

In `crates/aetherd/src/cli.rs`:
- Add `Enhance` variant to the `Commands` enum with args from the spec
- Add `--clipboard`, `--rewrite`, `--output`, `--budget`, `--offline` flags

In `crates/aetherd/src/main.rs`:
- Add command dispatch for `Enhance`
- Open store read-only
- Call `enhance_prompt_core()`
- Handle output: print to stdout, copy to clipboard if `--clipboard`, or JSON if `--output json`

For clipboard: use `arboard` crate. If unavailable at runtime, print the enhanced
prompt to stdout and show a message about installing clipboard support.

### Step 4: MCP tool (`crates/aether-mcp/src/tools/enhance.rs`)

Create `aether_enhance_prompt` MCP tool:
- Request: `prompt`, `budget` (default 8000), `rewrite` (default false), `format` (default "text")
- Response: `enhanced_prompt`, `resolved_symbols`, `referenced_files`, `rewrite_used`, `token_count`, `warnings`
- Reuses `enhance_prompt_core()` — the MCP tool is a thin wrapper
- Note: the MCP crate may not have direct access to the inference provider for
  the LLM calls. If so, use the keyword extraction fallback for the MCP tool
  and document that `--rewrite` requires the CLI. Alternatively, check if
  `SharedState` already has an inference provider reference.

In `crates/aether-mcp/src/tools/mod.rs`: add `pub mod enhance;`
In `crates/aether-mcp/src/tools/router.rs`: register the tool

### Step 5: Tests

In `crates/aetherd/src/enhance.rs` (or separate test file):
1. Test keyword extraction: "fix the login bug" extracts "login" as concept, "BugFix" as task type
2. Test template rendering: given mock context, output contains expected sections
3. Test empty workspace: returns original prompt with warning
4. Test token budget: large context gets truncated

In `crates/aether-mcp/src/tools/enhance.rs`:
1. Test MCP request/response serialization
2. Test enhance with no symbols found returns original prompt + warning

## SCOPE GUARD

**New files:**
- `crates/aetherd/src/enhance.rs`
- `crates/aetherd/src/enhance_templates.rs`
- `crates/aether-mcp/src/tools/enhance.rs`

**Modified files:**
- `crates/aetherd/src/cli.rs` (add Enhance subcommand)
- `crates/aetherd/src/main.rs` (add command dispatch)
- `crates/aetherd/src/lib.rs` or `mod.rs` (declare modules)
- `crates/aetherd/Cargo.toml` (add `arboard` if needed)
- `crates/aether-mcp/src/tools/mod.rs` (declare module)
- `crates/aether-mcp/src/tools/router.rs` (register tool)

Do NOT modify store schema, inference providers, or any analysis crate.
This feature reads existing data only — it does not write.

## VALIDATION GATE

```bash
cargo fmt --all --check
cargo clippy -p aetherd -p aether-mcp -- -D warnings
cargo test -p aetherd
cargo test -p aether-mcp
```

Do NOT run `cargo test --workspace` — OOM risk.

All commands must pass before committing.

## COMMIT

```bash
git add -A
git commit -m "feat: aether enhance — prompt enhancer with codebase intelligence context"
```

**PR title:** `feat: aether enhance — prompt enhancer with codebase intelligence context`

**PR body:**
```
Stage CC.6 of the Claude Code Integration phase.

Adds prompt enhancement that takes a vague coding prompt and enriches it
with AETHER's codebase intelligence:

- Intent extraction via triage LLM (with offline keyword fallback)
- Context assembly: SIR annotations, dependencies, callers, health scores,
  drift warnings, coupling data
- Template-based output (default) with optional LLM rewrite mode (--rewrite)
- CLI: `aether enhance "prompt" [--rewrite] [--clipboard] [--output json]`
- MCP: `aether_enhance_prompt` tool for agent integration
- Configurable token budget (default 8000)
- Offline mode (--offline) for keyword-only extraction without LLM

Inspired by Augment Code's Prompt Enhancer, differentiated by AETHER's
semantic intelligence depth (SIR, graph, coupling, health, drift).
```

## POST-COMMIT

```bash
git push origin feature/cc6-prompt-enhancer
# Create PR via GitHub web UI with title + body above
# After merge:
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/cc6-prompt-enhancer
git branch -D feature/cc6-prompt-enhancer
```
