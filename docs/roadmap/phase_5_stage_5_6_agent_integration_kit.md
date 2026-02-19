# Phase 5 - Stage 5.6: Agent Integration Kit

## Purpose
Ship the "last mile" that turns AETHER from a tool developers invoke manually into infrastructure that makes every AI coding agent in a project smarter by default. Today the MCP server exists and works, but no documentation or scaffolding tells agents *when* or *how* to use it effectively. This stage creates generated agent configuration files, a behavioral skill, a CLI scaffolding command, and a README section — so that `aether init-agent` gives any user a working AI-integrated setup in seconds.

## Current implementation (what exists)
- MCP server is registered with one CLI command (`claude mcp add ...`)
- MCP tools are stable with versioned response schemas (`schema_version`)
- CODEX_GUIDE documents the developer workflow for building AETHER itself
- `.agents/skills/aether-build` exists as a build-settings skill (internal, not user-facing)
- No user-facing agent configuration files ship with AETHER
- No guidance exists for agents on when/how to call AETHER tools
- CLI is entirely flag-based (`--workspace`, `--lsp`, `--search`, `--download-models`, `--calibrate`, etc.) — no subcommands exist yet

## Target implementation
- `aether init-agent` CLI subcommand scaffolds agent config files into the user's project
- Reference `CLAUDE.md` for Claude Code (primary target)
- Reference `.codex-instructions` for OpenAI Codex CLI (secondary)
- Reference `.cursor/rules` for Cursor (secondary)
- AETHER coding skill at `.agents/skills/aether-context/SKILL.md`
- README section on agent integration
- Agent config template versioning via `AETHER_AGENT_SCHEMA_VERSION` constant
- **This stage introduces subcommand parsing to the CLI** — `init-agent` has its own sub-flags (`--platform`, `--force`) that don't fit the existing top-level flag pattern

## In scope
- **Refactor `aetherd` CLI to support subcommands alongside existing flags:**
  - Existing flags (`--workspace`, `--lsp`, `--search`, `--download-models`, `--calibrate`, etc.) continue to work unchanged as "default mode" (no subcommand)
  - New `init-agent` subcommand added with its own flags
  - Use clap's subcommand support — existing flag-based usage becomes the implicit default when no subcommand is given
  - This refactor is minimal: wrap existing flag parsing in a default arm, add `init-agent` as a named subcommand
- Add `init-agent` subcommand to `aetherd` CLI in `crates/aetherd`
- Template files for Claude Code, Codex, and Cursor stored in `crates/aetherd/src/templates/`
- Templates are generated (not static copies) — adapt to the user's current `.aether/config.toml`:
  - Which inference provider is configured
  - Which languages are enabled
  - Which verify commands are set
  - Whether semantic search / embeddings are enabled
- AETHER coding skill with tiered behavioral guidance
- Agent schema version constant in `crates/aether-core`
- README.md update with "Agent Integration" section
- Docs update: this stage doc + DECISIONS entry

## Out of scope
- Hook integration (deferred — no performance data yet, advisory approach covers value)
- Auto-updating agent configs when AETHER config changes (user re-runs `init-agent`)
- Agent configs for platforms beyond Claude Code, Codex, and Cursor
- Staleness mitigation beyond advisory guidance (staging buffer is Phase 6+)
- MCP tool changes (all tools are stable from prior stages)
- Full CLI migration to subcommand-only (existing flags preserved as default mode)

## Locked decisions

### 30. Tiered agent guidance — mandatory for destructive, advisory for routine
Agent config files use a two-tier approach:
- **Mandatory tier:** "Always call `aether_get_sir` before reverting, deleting, or refactoring symbols. Always call `aether_why_changed` before reverting recent changes. Always call `aether_verify` after modifying code."
- **Advisory tier:** "Consider calling `aether_search` when exploring unfamiliar code. Consider calling `aether_symbol_timeline` when reviewing recent changes."

This balances safety (agents don't blindly break things) with speed (agents aren't forced to make tool calls for trivial edits).

### 31. Hooks deferred — advisory-first approach
Agent hooks (pre-edit, post-edit automation) are deferred until real usage data shows where agents consistently skip AETHER. The `CLAUDE.md` advisory approach covers most value without performance risk. Hooks are a candidate for Phase 6.

### 32. Claude Code is primary target
Claude Code gets the richest integration: `CLAUDE.md` + skill. Codex and Cursor get equivalent `CLAUDE.md`-style files adapted to their formats. All three share ~90% content with platform-specific formatting.

### 33. Generated agent configs via `aether init-agent`
Agent config files are generated from templates, not static. The generator reads `.aether/config.toml` and adapts the output:
- Lists the active languages (Rust, TypeScript, Python, etc.)
- Lists the configured verify commands
- Notes whether semantic search is available
- Includes the correct MCP binary path for the platform
The user can re-run `aether init-agent` after config changes to update.

### 34. Agent config schema versioning
`AETHER_AGENT_SCHEMA_VERSION` constant in `crates/aether-core/src/lib.rs` (or `types.rs`). Bumped when new MCP tools are added or existing tool contracts change. Templates embed this version. Users can compare their generated files to the current version and re-run `init-agent` if stale.

### 35. Subcommand introduction — backward-compatible CLI refactor
This stage introduces clap subcommand parsing. The `init-agent` command has its own sub-flags (`--platform`, `--force`) that don't fit the existing top-level flag pattern. The refactor is minimal:
- Existing flag-based usage (`aetherd --workspace . --lsp`, `aetherd --workspace . --search "query"`, etc.) continues to work as the default mode (no subcommand specified)
- `init-agent` is added as a named subcommand with its own argument group
- Future stages (5.7 `setup-local`) will add additional subcommands following this same pattern

## Implementation notes

### CLI command: `aether init-agent`

```
aetherd --workspace . init-agent [--platform <claude|codex|cursor|all>] [--force]
```

- Default `--platform all` generates files for all three platforms
- `--force` overwrites existing files (default: skip with warning if file exists)
- Reads `.aether/config.toml` for dynamic template values
- If `.aether/config.toml` doesn't exist, runs with sensible defaults and warns
- Exit codes: 0 = success, 1 = error, 2 = files already exist (no `--force`)

### CLI parser refactor pattern

```rust
// Before (5.5 and earlier): all top-level flags
#[derive(Parser)]
struct Cli {
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    lsp: bool,
    #[arg(long)]
    search: Option<String>,
    #[arg(long)]
    download_models: bool,
    #[arg(long)]
    calibrate: bool,
    // ... etc
}

// After (5.6): subcommands + backward-compatible default mode
#[derive(Parser)]
struct Cli {
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,

    // Existing flags preserved for default mode (no subcommand)
    #[arg(long)]
    lsp: bool,
    #[arg(long)]
    search: Option<String>,
    #[arg(long)]
    download_models: bool,
    #[arg(long)]
    calibrate: bool,
    // ... etc
}

#[derive(Subcommand)]
enum Commands {
    /// Generate agent configuration files for AI coding agents
    InitAgent {
        #[arg(long, default_value = "all")]
        platform: String,
        #[arg(long)]
        force: bool,
    },
}
```

When `command` is `None`, the existing flag-based logic runs unchanged. When `command` is `Some(Commands::InitAgent { .. })`, the init-agent logic runs. This ensures zero breaking changes to existing usage.

### Generated file locations

| Platform | File | Location |
|----------|------|----------|
| Claude Code | `CLAUDE.md` | `<workspace>/CLAUDE.md` |
| Claude Code | AETHER skill | `<workspace>/.agents/skills/aether-context/SKILL.md` |
| Codex | Instructions | `<workspace>/.codex-instructions` |
| Cursor | Rules | `<workspace>/.cursor/rules` |

### CLAUDE.md template structure

```markdown
# AETHER Code Intelligence

This project uses AETHER for semantic code understanding. The MCP server
provides structured context about every symbol in the codebase.

## Agent Schema Version: {AETHER_AGENT_SCHEMA_VERSION}

## Available Tools
{dynamic: list MCP tools with one-line descriptions}

## Available Languages
{dynamic: list from config, e.g. "Rust, TypeScript, Python"}

## Search Modes
{dynamic: if embeddings enabled, list all three modes; otherwise note lexical-only}

## Required Actions (always do these)
- Before reverting or deleting code: call `aether_why_changed` to understand
  the intent behind recent changes
- Before refactoring a symbol: call `aether_get_sir` to check for side effects,
  error modes, and dependencies that must be preserved
- After modifying code: call `aether_verify` to run the project test suite
- If `aether_verify` fails: fix the issue before proceeding

## Recommended Actions (do these when helpful)
- When exploring unfamiliar code: call `aether_search` with hybrid mode to
  find relevant symbols by meaning, not just name
- When reviewing recent changes: call `aether_symbol_timeline` to see how
  a symbol's intent evolved over time
- When tracing call chains: call `aether_call_chain` to understand what
  calls what, up to configurable depth
- At the start of a task: call `aether_status` to confirm the index is fresh

## Staleness Note
If you've made many rapid edits, the AETHER index may be slightly behind.
Call `aether_status` to check `symbol_count` and `sir_count`. If the index
seems stale, wait briefly for re-indexing or trigger `aether_index_once`
before trusting search results for recently-modified code.

## Verify Commands
{dynamic: list from config.verify.commands, e.g. "cargo test", "cargo clippy"}
```

### Codex and Cursor templates

The `.codex-instructions` and `.cursor/rules` files contain the same core guidance (required actions, recommended actions, staleness note) reformatted for each platform's conventions:

- **Codex:** Plain text, imperative style. No markdown headers (Codex reads raw text). Skill references use `$aether-context` syntax.
- **Cursor:** Markdown-compatible. Uses Cursor's `@` mention syntax where applicable. Rules are structured as numbered directives.

Content is ~90% shared. The platform-specific delta is formatting and reference syntax only.

### AETHER coding skill (`SKILL.md`)

```markdown
---
name: aether-context
description: "Code intelligence workflow for AETHER-indexed projects. Use before
modifying, refactoring, or reviewing unfamiliar code."
---

# AETHER Code Intelligence Workflow

## When to activate
- Any task involving code modification, refactoring, or review
- Onboarding to unfamiliar modules or symbols
- Investigating why code was changed recently
- Tracing dependencies or call chains before making changes

## Workflow: Orient → Discover → Understand → Modify → Verify

### 1. Orient
Call `aether_status` to confirm the index is current.
Check `symbol_count` and `sir_count` — if they seem low, the index may
still be building.

### 2. Discover
Call `aether_search` with `--search-mode hybrid` to find symbols related
to your task. Hybrid mode combines name matching with semantic meaning.
If semantic search is unavailable, AETHER falls back to lexical automatically.

### 3. Understand
For each symbol you plan to modify:
- Call `aether_get_sir` — read the `intent`, `side_effects`, `error_modes`,
  and `dependencies` fields carefully
- Call `aether_symbol_timeline` if the symbol was recently changed
- Call `aether_call_chain` to see what depends on this symbol

### 4. Modify
Make your changes with full context. Preserve:
- Side effects listed in SIR (these are often invisible in raw source)
- Error modes (don't silently swallow errors that callers expect)
- Dependency contracts (if X depends on Y's return type, changing Y breaks X)

### 5. Verify
Call `aether_verify` to run the project's configured test/lint commands.
If verification fails, fix before proceeding.

## Anti-patterns to avoid
- Don't grep raw source when `aether_search --search-mode hybrid` exists
- Don't guess at side effects — `aether_get_sir` tells you explicitly
- Don't revert recent changes without `aether_why_changed` first
- Don't assume the index is fresh after bulk edits — check `aether_status`
```

### Template rendering

Templates are Rust string literals with `{placeholder}` substitution in `crates/aetherd/src/templates/`. No external template engine dependency — the templates are simple enough for `format!()` or a minimal handwritten renderer.

Dynamic values read from config:
- `languages`: collected from `LanguageRegistry::supported_languages()` (reflects what's compiled in)
- `verify_commands`: from `config.verify.commands`
- `search_modes`: conditional on `config.embeddings.enabled`
- `inference_provider`: from `config.inference.provider`
- `agent_schema_version`: from `AETHER_AGENT_SCHEMA_VERSION` constant
- `mcp_binary_path`: resolved from the built binary location or config override

### README update

Add a new "Agent Integration" section after the existing "MCP Server" section:

```markdown
## Agent Integration

AETHER can generate configuration files that teach AI coding agents
how to use your codebase intelligence effectively.

### Quick Setup

```bash
# Generate agent config files for all supported platforms
aetherd --workspace . init-agent

# Generate for a specific platform only
aetherd --workspace . init-agent --platform claude
```

This creates:
- `CLAUDE.md` — behavioral guidance for Claude Code
- `.agents/skills/aether-context/SKILL.md` — detailed coding workflow
- `.codex-instructions` — guidance for OpenAI Codex CLI
- `.cursor/rules` — guidance for Cursor

Files are generated from your `.aether/config.toml`, so they reflect
your actual setup (languages, verify commands, search capabilities).

### Updating After Config Changes

Re-run `init-agent --force` after changing your AETHER configuration
to regenerate agent files with updated settings.
```

## Edge cases

| Scenario | Behavior |
|----------|----------|
| `.aether/config.toml` doesn't exist | Generate with defaults, warn user to run `aetherd --workspace . --index-once` first |
| `CLAUDE.md` already exists (user-written) | Skip with warning, suggest `--force` to overwrite |
| `.cursor/` directory doesn't exist | Create it |
| `.agents/skills/` directory doesn't exist | Create it |
| No verify commands configured | Omit verify section from templates, note "no verify commands configured" |
| Embeddings not enabled | Templates note "lexical search only — enable embeddings for semantic search" |
| User runs `init-agent` with `--platform claude` only | Only generate Claude Code files, skip Codex and Cursor |
| MCP binary not found at expected path | Template includes placeholder path with comment to update |
| Workspace has no supported language files | Templates still generate (user may add files later), note "no indexed languages detected" |
| User runs old flag-based commands after CLI refactor | Work unchanged — subcommand is `Option`, `None` means default flag-based mode |

## Pass criteria
1. `aetherd --workspace . init-agent` creates `CLAUDE.md`, `.codex-instructions`, `.cursor/rules`, and `.agents/skills/aether-context/SKILL.md` in the workspace root.
2. Generated `CLAUDE.md` includes dynamic values from `.aether/config.toml` (languages, verify commands, search modes).
3. `--platform claude` generates only Claude Code files.
4. `--platform all` generates files for all three platforms.
5. Existing files are not overwritten without `--force`.
6. `--force` overwrites existing files.
7. Generated files include `AETHER_AGENT_SCHEMA_VERSION`.
8. `AETHER_AGENT_SCHEMA_VERSION` constant exists in `crates/aether-core`.
9. README.md has an "Agent Integration" section with usage instructions.
10. **Existing flag-based CLI usage (`--lsp`, `--search`, `--download-models`, `--calibrate`, etc.) continues to work unchanged after the subcommand refactor.**
11. `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` pass.

## Exact Codex prompt(s)
```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- CARGO_TARGET_DIR=/home/rephu/aether-target
- CARGO_BUILD_JOBS=2
- PROTOC=$(which protoc)
- Do NOT use /tmp/ for any build artifacts — /tmp/ is RAM-backed (tmpfs) in WSL2.

NOTE ON CLI ARCHITECTURE: The current aetherd CLI uses ONLY top-level flags
(--workspace, --lsp, --search, --download-models, --calibrate, etc.) with NO
subcommands. This stage introduces subcommand parsing for the first time.
Use clap's subcommand support to add `init-agent` as a subcommand while keeping
ALL existing flags working as the default mode (when no subcommand is given).
The subcommand field should be Option<Commands> — None means run existing
flag-based logic unchanged. This is a BACKWARD-COMPATIBLE refactor.

You are working in the repo root of https://github.com/rephug/aether.

Read these files for context first:
- docs/roadmap/phase_5_stage_5_6_agent_integration_kit.md (this file)
- crates/aetherd/src/main.rs (CLI entry point — currently flag-only, needs subcommand refactor)
- crates/aether-config/src/lib.rs (config loading)
- crates/aether-core/src/lib.rs (constants)
- crates/aether-mcp/src/lib.rs (MCP tool names for template reference)
- README.md (for adding Agent Integration section)

1) Ensure working tree is clean. If not, stop and report dirty files.
2) Create branch feature/phase5-stage5-6-agent-integration-kit off main.
3) Create worktree ../aether-phase5-stage5-6-agent-kit for that branch and switch into it.
4) FIRST: Refactor crates/aetherd/src/main.rs CLI parser:
   - Add `#[command(subcommand)] command: Option<Commands>` to the Cli struct
   - Move --workspace to a global arg (used by both default mode and subcommands)
   - Create `enum Commands` with `InitAgent` variant
   - In main(), match on cli.command: None → existing flag logic, Some(InitAgent) → new init-agent logic
   - Verify ALL existing flags still work by running existing tests
5) Add AETHER_AGENT_SCHEMA_VERSION constant (u32, initial value 1) in crates/aether-core/src/lib.rs.
6) Create template module at crates/aetherd/src/templates/mod.rs:
   - Template structs for Claude, Codex, Cursor, and Skill
   - Each template takes a TemplateContext struct with: languages, verify_commands,
     embeddings_enabled, inference_provider, agent_schema_version, mcp_binary_hint
   - Render methods return String content for each file
   - Templates use format!() string substitution — no external template engine
7) Create crates/aetherd/src/templates/claude_md.rs:
   - CLAUDE.md template with Required Actions (mandatory tier) and
     Recommended Actions (advisory tier)
   - Dynamic sections for languages, search modes, verify commands
   - Staleness guidance note
8) Create crates/aetherd/src/templates/codex_instructions.rs:
   - Same core content as CLAUDE.md, reformatted as plain text
9) Create crates/aetherd/src/templates/cursor_rules.rs:
   - Same core content as CLAUDE.md, reformatted as numbered directives
10) Create crates/aetherd/src/templates/skill_md.rs:
    - AETHER coding skill with Orient → Discover → Understand → Modify → Verify workflow
    - Anti-patterns section
11) Wire `init-agent` subcommand to template rendering:
    - Flags: --platform <claude|codex|cursor|all> (default: all), --force (default: false)
    - Read .aether/config.toml for dynamic values (handle missing config gracefully)
    - Build TemplateContext from config
    - Write files to workspace root, respecting --force flag
    - Create directories (.cursor/, .agents/skills/aether-context/) as needed
    - Exit 0 on success, 1 on error, 2 if files exist and no --force
12) Add README.md "Agent Integration" section after the "MCP Server" section.
13) Add tests:
    - Template rendering produces expected content with known TemplateContext values
    - init-agent creates all expected files in a temp workspace
    - init-agent skips existing files without --force
    - init-agent overwrites with --force
    - Generated CLAUDE.md contains the schema version string
    - Generated files reflect config values (e.g., if embeddings enabled, mentions semantic search)
    - REGRESSION: existing flag-based CLI usage still works (--lsp, --search, etc.)
14) Run:
    - cargo fmt --all --check
    - cargo clippy --workspace -- -D warnings
    - cargo test --workspace
15) Commit with message: "Add agent integration kit with init-agent CLI subcommand".
```

## Expected commit
`Add agent integration kit with init-agent CLI subcommand`
