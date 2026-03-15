# Phase Repo — The Prompter

**Codename:** The Prompter
**Thesis:** AETHER already knows your codebase better than any context-engineering tool. This phase makes that intelligence *portable* — exportable to any AI chat, any agent, any clipboard — so developers can carry AETHER's understanding into any conversation, not just MCP-connected ones.

**Inspiration:** RepoPrompt demonstrated that developers want structured context assembly for AI conversations. AETHER already has deeper intelligence (SIR, graph, coupling, drift, health) but currently only exposes it through MCP tools and CLI queries. Phase Repo bridges the gap: AETHER's intelligence, formatted for human-pasted or agent-consumed context windows.

**One-sentence summary:** "Your codebase intelligence, anywhere you need it."

---

## Why This Phase, Why Now

1. **Immediate daily value.** Robert's workflow involves consulting Claude, Gemini Deep Think, and ChatGPT — none of which are MCP-connected. Today that means manually assembling context. Phase Repo automates this.

2. **Low implementation cost.** The `sir-context` command already exists in `aetherd` with budgeted symbol-centric context assembly. Phase Repo generalizes that engine to support file targets, overview mode, and multiple output formats. No new crates, no new databases, no new protocols.

3. **Amplifies existing investment.** Phase 8 built a world-class understanding engine. Phase 10 makes it autonomous. Phase Repo makes it *accessible* — the missing bridge between intelligence and consumption.

4. **Competitive moat.** RepoPrompt's code maps are shallow structural overviews. AETHER's context export includes semantic intent, dependency graphs, coupling data, health scores, and drift warnings. No other tool can produce context this rich.

---

## Existing Foundation: `sir-context`

The current `sir-context` command in `aetherd` provides symbol-centric budgeted context assembly. Phase Repo does **not** create a parallel pipeline. Instead, R.1 refactors `sir_context.rs` into a shared assembly core that both the new `context` command and the compatibility `sir-context` alias use.

**What already exists:**
- Greedy token allocator with tier-based priority
- Symbol selector resolution (by ID, qualified name, or fuzzy name match)
- SIR loading and formatting
- Caller/dependency graph traversal via SQLite graph APIs
- Test intent queries
- Project memory search via `aether_recall`

**What Phase Repo adds:**
- File-level targeting (not just symbol-level)
- Overview mode (project-level summary)
- Additional intelligence layers (coupling, health, drift)
- Multiple output formats (markdown, JSON, XML, compact)
- Preset system for reusable configurations
- Interactive dashboard builder

---

## Stage Plan

| Stage | Name | Scope | Codex Runs | Dependencies |
|-------|------|-------|------------|--------------|
| R.1 | Context Export CLI | `aether context` command — generalize `sir-context` into shared export engine | 1–2 | Phase 8 complete (SIR, graph, health) |
| R.2 | SIR-Guided File Slicing | Symbol-range extraction for token-efficient file inclusion | 1 | R.1 |
| R.3 | Prompt Preset Library | Named, reusable prompt templates with variable substitution | 1–2 | R.1 |
| R.4 | Multi-Format Output | XML and compact formatters on top of the shared ExportDocument | 1 | R.1 |
| R.5 | Interactive Context Builder | Dashboard page with live token counting and clipboard export (no TUI) | 1–2 | R.1, R.2 |

### Dependency Chain

```
R.1 (Context Export) ──► R.2 (File Slicing)
        │                      │
        ├──► R.3 (Presets)     │
        │                      │
        ├──► R.4 (Formats)     │
        │                      │
        └──► R.5 (Dashboard) ◄─┘
```

R.2, R.3, and R.4 are independent after R.1. R.5 benefits from R.2 but can start without it.

**Priority order for implementation:** R.1 → R.2 → R.3 → R.4 → R.5

---

## Decisions to Lock for Phase Repo

| # | Decision | Resolution | Rationale |
|---|----------|------------|-----------|
| 97 | Command surface | New top-level `context` subcommand; `sir-context` retained as compatibility alias routing to the same shared engine. | `context` is cleaner for daily human use. `sir-context` stays so nothing that references it breaks. Zero migration cost. |
| 98 | Default token budget for export: 32K tokens | Same as Phase 8 context assembly. Configurable via `--budget` flag. | 32K fits in every major model's context window. Users can raise for 128K+ models. |
| 99 | Default output: markdown to stdout | `aether context` prints to stdout. Pipe to `pbcopy`/`xclip`/`xsel` for clipboard. `--output` flag for file write parity with current `sir-context`. No built-in clipboard dependency. | Cross-platform without linking to platform clipboard libraries. Shell piping is universal. |
| 100 | Presets stored in `.aether/presets/` as TOML files | One file per preset. Human-editable. Version-controllable. | TOML is already the config format. No new parser needed. Users can share presets by committing the directory. |
| 101 | File slicing granularity: symbol-level | Slice at symbol boundaries (function, struct, impl block, etc.) using existing tree-sitter span data from `symbols` table. Not line-level or AST-node-level. | Symbol-level matches SIR granularity. AETHER already knows every symbol's byte range from parsing. |
| 102 | R.1 ships markdown + JSON; XML and compact deferred to R.4 | Markdown is the primary paste-into-chat format. JSON is free via serde. XML/compact are rendering work that belongs in R.4. | Keeps R.1 focused on the assembly engine and the two highest-value formats. |

---

## Stage R.1 — Context Export CLI

**Codename:** Courier
**Depends on:** Phase 8 complete (SIR store, graph, health scores)
**New crates:** None
**Modified crates:** `aetherd` (new subcommand + refactored `sir_context.rs`)

### Purpose

Add a top-level `aether context` CLI command that assembles AETHER's intelligence into a single, token-budgeted document suitable for pasting into any AI chat interface. The implementation generalizes the existing `sir-context` engine into a shared assembly core used by both commands.

### Architecture: Shared Export Engine

Refactor current `sir_context.rs` into two layers:

1. **Assembly core** — resolve targets, load indexed data, apply budget tiers, emit `ExportDocument`
2. **Renderers** — markdown and JSON (R.1), XML and compact added in R.4

Both `context` and `sir-context` route through the assembly core. `sir-context` maps its existing symbol-selector + file arguments into a `ContextTarget::Symbol` and calls the same engine.

### Shared Export Model

```rust
/// What to export context for.
pub enum ContextTarget {
    /// File-level: include all symbols in this file + neighborhood
    File { path: PathBuf },
    /// Symbol-level: focused on a specific symbol (current sir-context behavior)
    Symbol { selector: String, file_hint: Option<PathBuf> },
    /// Project overview: aggregate summary without specific targets
    Overview,
}

/// Which intelligence layers to include.
pub struct LayerSelection {
    pub sir: bool,           // SIR annotations for target + neighbor symbols
    pub source: bool,        // Actual source code (whole files in R.1, sliced in R.2)
    pub graph: bool,         // Dependency neighborhood (callers, callees)
    pub coupling: bool,      // Co-change coupling data
    pub health: bool,        // Health warnings and scores
    pub drift: bool,         // Active drift alerts
    pub memory: bool,        // Relevant project memory notes
    pub tests: bool,         // Test intents for target symbols
}

/// Output format selection.
pub enum ContextFormat {
    Markdown,   // R.1: human-paste format
    Json,       // R.1: programmatic consumption
    Xml,        // R.4: structured Claude API format
    Compact,    // R.4: maximum density
}

/// The assembled export document, format-agnostic.
pub struct ExportDocument {
    /// Project-level overview (symbol count, SIR coverage, health)
    pub project_overview: ProjectOverview,
    /// Per-target sections with symbols, SIRs, source
    pub target_sections: Vec<TargetSection>,
    /// Neighbor SIR summaries from graph traversal
    pub neighbor_summaries: Vec<NeighborSummary>,
    /// Per-layer budget usage tracking
    pub budget_usage: BudgetUsage,
    /// Notices (e.g., "coupling data unavailable — SurrealKV locked")
    pub notices: Vec<String>,
}
```

### CLI Interface

```bash
# File-level context (most common use case)
aether context crates/aether-store/src/lib.rs

# Symbol-level context
aether context --symbol GraphStore --file crates/aether-store/src/graph.rs

# Multiple file targets
aether context crates/aether-mcp/src/lib.rs crates/aether-infer/src/lib.rs

# Project overview
aether context --overview

# Budget and depth control
aether context crates/aether-store/src/lib.rs --budget 64000 --depth 3

# Layer control
aether context crates/aether-mcp/src/lib.rs --include sir,graph,coupling,health
aether context crates/aether-mcp/src/lib.rs --exclude drift,memory

# Format selection (R.1: markdown or json)
aether context crates/aether-store/src/lib.rs --format markdown
aether context crates/aether-store/src/lib.rs --format json

# Task-oriented bias for smarter context selection
aether context --task "refactor the SIR pipeline into smaller modules" crates/aetherd/src/sir_pipeline.rs

# Write to file (parity with current sir-context)
aether context crates/aether-store/src/lib.rs --output context.md

# Pipe to clipboard
aether context crates/aether-store/src/lib.rs | xclip -selection clipboard
aether context crates/aether-store/src/lib.rs | pbcopy  # macOS

# Compatibility: sir-context still works, routes to same engine
aether sir-context --selector SharedState --file crates/aetherd/src/main.rs
```

### Target Resolution Rules

| Target Type | Resolution | Fallback |
|---|---|---|
| Indexed file | `list_symbols_for_file()` → assemble per-file sections with SIR, source, neighbors | — |
| Unindexed or no-SIR file | Read file from disk, emit source-only section | Add "index data unavailable" notice |
| Empty workspace | Detect via `count_symbols_with_sir()` | Source-only output + "run aetherd --index-once first" notice |
| Symbol target | Reuse current selector resolution logic from `sir-context` | — |
| Overview | `count_symbols_with_sir()` + aggregate health/drift summaries | Health/drift sections omit cleanly if no data exists |

### Data Sources

| Layer | Source | Unavailable Behavior |
|---|---|---|
| Symbols + SIR | SQLite `symbols` + `sir_annotations` tables | Notice: "index data unavailable" |
| Graph (callers/deps) | SQLite graph APIs (`callers_of`, `dependencies_of`) | Omit graph section |
| Coupling | SurrealDB readonly access (best effort) | Omit coupling + notice: "coupling data unavailable (SurrealKV locked)" |
| Health | `HealthAnalyzer` filtered to target symbols/files | Fall back to symbol risk/staleness metadata + notice |
| Drift | SQLite `drift_results` table | Omit drift section |
| Memory | SQLite project notes via semantic search | Omit memory section |
| Tests | SQLite `test_intents` table | Omit tests section |
| Source code | Disk read of target files | Error if file doesn't exist |

**Key principle:** SurrealKV lock contention is a known issue. The context command must never fail because SurrealDB is locked by a running `aetherd` process. Coupling data is best-effort; all other layers use SQLite which supports concurrent readers.

### Budget Policy

Reuse the existing greedy token allocator. Updated tier order for Phase Repo priorities:

| Priority | Layer | Default % of Budget | Rationale |
|----------|-------|-------------------|-----------|
| 1 | Target file source | 30% | The AI needs to see the actual code |
| 2 | Target symbol SIRs | 15% | Semantic understanding of what the code does |
| 3 | Immediate neighbor SIRs | 15% | Context for how target fits into the system |
| 4 | Test intents | 10% | What's tested, what's not |
| 5 | Coupling data | 8% | What files change together |
| 6 | Project memory | 7% | Architectural decisions, past context |
| 7 | Health warnings | 5% | Known issues with these symbols |
| 8 | Drift alerts | 5% | Where code and intent have diverged |
| 9 | Broader graph | 5% | Extended dependency neighborhood |

When budget is exhausted, lower-priority layers are truncated or omitted. The output includes a budget usage footer showing what was included vs. truncated.

### Markdown Output Example (Real AETHER Symbols)

```markdown
# AETHER Context: crates/aether-store/src/graph.rs
Generated: 2026-03-15T14:30:00Z | Budget: 32,000 tokens | Used: 24,180 tokens

## Project Overview
- **Workspace:** /home/rephu/projects/aether
- **Total Symbols:** 3,748 | **SIR Coverage:** 100%
- **Health Score:** 42/100 (Watch)

## Target File: crates/aether-store/src/graph.rs
### Symbols (12)

#### `GraphStore` (Trait)
**Intent:** Defines the abstract interface for persisting and querying
dependency edges between symbols. Supports CALLS, DEPENDS_ON, TYPE_REF,
and IMPLEMENTS edge types with bidirectional traversal.
**Edge Cases:** Empty graph returns empty vecs, duplicate edges are idempotent
**Dependencies:** SymbolId, EdgeKind, GraphEdge
**Health:** 82/100 | **Drift:** None

#### `SurrealGraphStore` (Struct)
**Intent:** SurrealDB-backed implementation of GraphStore using Record
References for bidirectional edge traversal and SurrealKV for embedded storage.
...

### File Source
```rust
// crates/aether-store/src/graph.rs (lines 1-340)
use crate::edge::{EdgeKind, GraphEdge};
...
```

## Dependency Neighborhood (2-hop)

### Callers of target symbols
| Symbol | File | Relationship |
|--------|------|-------------|
| `build_dependency_graph` | crates/aetherd/src/indexer.rs | CALLS → GraphStore::upsert_edges |
| `health_analysis` | crates/aether-analysis/src/health.rs | CALLS → GraphStore::callers_of |

### Callees from target symbols
| Symbol | File | Relationship |
|--------|------|-------------|
| `SymbolId::from_parts` | crates/aether-core/src/symbol.rs | SurrealGraphStore → CALLS |

### Neighbor SIRs (summarized)
**`build_dependency_graph`** — Traverses tree-sitter AST to extract CALLS and
DEPENDS_ON edges, batches them, and upserts via GraphStore trait...

## Coupling Data
| File | Coupling Score | Signal |
|------|---------------|--------|
| crates/aether-store/src/edge.rs | 0.91 | Co-change in 14/16 commits |
| crates/aether-analysis/src/health.rs | 0.58 | Shared dependency pattern |

## Health Warnings
- **God File risk:** graph.rs at 340 lines is within bounds but growing
- **Test Coverage Gap:** GraphStore trait has 4 edge types but only 2 have dedicated test coverage

## Active Drift
(None detected for target symbols)

## Relevant Project Memory
- "SurrealDB 3.0 replaces CozoDB — Decision #38, Phase 7.2 migration" (2026-02-21)
- "Record References for bidirectional edges — Decision #42" (2026-02-21)

## Budget Usage
| Layer | Tokens | Status |
|-------|--------|--------|
| Source | 7,200 | ✓ Included |
| Target SIRs | 3,600 | ✓ Included |
| Neighbor SIRs | 4,100 | ✓ Included |
| Test Intents | 2,400 | ✓ Included |
| Coupling | 1,800 | ✓ Included |
| Memory | 1,680 | ✓ Included |
| Health | 1,200 | ✓ Included |
| Drift | 200 | ✓ Included |
| Broader Graph | 2,000 | ✓ Included |
| **Total** | **24,180 / 32,000** | |
```

### Compatibility: sir-context Mapping

`sir-context` stays as a compatibility alias. It maps its existing arguments into the shared engine:

```
aether sir-context --selector SharedState --file crates/aetherd/src/main.rs
  → ContextTarget::Symbol { selector: "SharedState", file_hint: Some("crates/aetherd/src/main.rs") }
  → LayerSelection::default()
  → ContextFormat::Markdown
```

Existing tests and docs that reference `sir-context` continue to work. New tests and docs use `context` as the canonical command.

### Pass Criteria

1. `aether context <file>` produces markdown output with SIR summaries, dependency neighborhood, coupling data, health warnings.
2. `aether context --symbol <n> --file <path>` produces symbol-focused output equivalent to current `sir-context`.
3. `sir-context` still works as compatibility alias routing through shared engine.
4. `--budget` flag correctly limits output size.
5. `--include` and `--exclude` flags control which intelligence layers appear.
6. `--depth` controls graph traversal depth.
7. `--format markdown` (default) and `--format json` both produce valid output.
8. `--overview` mode produces project-level summary without file targets.
9. `--task` mode biases context selection toward task-relevant symbols via semantic search.
10. Unindexed file degrades to source-only output with notice (not an error).
11. SurrealKV lock contention produces a notice, not a failure — coupling layer omitted gracefully.
12. Empty workspace degrades gracefully with "run aetherd --index-once first" notice.
13. Output includes budget usage footer (included/truncated/omitted layers).
14. `cargo fmt --all --check`, `cargo clippy -p aetherd`, `cargo test -p aetherd` pass.

### Exact Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_repo_the_prompter.md for the full specification.
Focus on Stage R.1 — Context Export CLI.

IMPORTANT CONTEXT: The existing `sir-context` command in aetherd already has
a budgeted, symbol-centric context assembly engine in sir_context.rs. Do NOT
create a parallel pipeline. Refactor sir_context.rs into two layers:
1. Assembly core: resolve targets, load indexed data, apply budget tiers, emit ExportDocument
2. Renderers: markdown and JSON formatters

Both the new `context` command and the compatibility `sir-context` alias
route through the same assembly core.

PREFLIGHT:
1) Ensure working tree is clean (git status --porcelain must be empty).
2) git pull --ff-only
3) git worktree add -b feature/phase-repo-stage-r1-context-export /home/rephu/phase-repo-r1
4) cd /home/rephu/phase-repo-r1

IMPLEMENTATION:

5) Refactor crates/aetherd sir_context.rs:
   - Extract shared types: ContextTarget (File/Symbol/Overview), LayerSelection,
     ExportDocument, BudgetUsage, ContextFormat
   - Extract assembly core: resolve targets → load data → apply budget → emit ExportDocument
   - Keep sir-context working by mapping its args into ContextTarget::Symbol

6) Add `context` subcommand to aetherd CLI:
   - Positional file targets → ContextTarget::File per path
   - --symbol with optional --file → ContextTarget::Symbol
   - --overview → ContextTarget::Overview
   - --budget (default 32000), --depth (default 2)
   - --include, --exclude (layer names: sir,source,graph,coupling,health,drift,memory,tests)
   - --format markdown|json
   - --task (string, passed to memory search and neighbor ordering)
   - --output (file path, parity with sir-context)
   - Default: markdown to stdout

7) Assembly core target resolution:
   - Indexed file: list_symbols_for_file() → per-file sections
   - Unindexed file: read from disk, source-only section + "index data unavailable" notice
   - Symbol: reuse existing selector resolution from sir-context
   - Overview: count_symbols_with_sir() + aggregate health/drift if available

8) Data source access:
   - SQLite for symbols, SIR, test intents, project notes, drift results, graph
   - SurrealDB for coupling: best-effort readonly. If locked, omit coupling + add notice
   - HealthAnalyzer for health warnings: filter to targets. Fallback to symbol metadata if unavailable
   - All layers degrade gracefully — never fail the whole command because one layer is unavailable

9) Budget tiers (greedy allocator, same pattern as existing sir-context):
   Priority 1: target source (30%)
   Priority 2: target SIRs (15%)
   Priority 3: immediate neighbor SIRs (15%)
   Priority 4: test intents (10%)
   Priority 5: coupling (8%)
   Priority 6: project memory (7%)
   Priority 7: health warnings (5%)
   Priority 8: drift alerts (5%)
   Priority 9: broader graph (5%)

10) Renderers:
    - MarkdownRenderer: headers, tables, code fences, budget footer
    - JsonRenderer: serde_json serialization of ExportDocument

11) Wire sir-context as compatibility alias:
    - Map existing selector/file args → ContextTarget::Symbol
    - Route through shared assembly core
    - Preserve current behavior exactly

12) Tests:
    - context with indexed file target: includes source, SIRs, neighbors, tests, memory
    - context with symbol target: matches current sir-context behavior
    - context --overview: project summary without file sections
    - Unindexed file: source-only output + notice
    - Empty workspace: graceful degradation
    - Coupling unavailable (SurrealKV locked): notice, not error
    - Health/drift sections omit cleanly when no data exists
    - Low budget: lower-priority layers truncated first, omissions recorded
    - --format json: valid JSON matching ExportDocument schema
    - sir-context compatibility: still parses and routes correctly

13) Run:
    - cargo fmt --all --check
    - cargo clippy -p aetherd -- -D warnings
    - cargo test -p aetherd
    - Per-crate only — never --workspace

14) Commit: "Add aether context CLI with shared export engine (Phase Repo R.1)"

PR title: "Phase Repo R.1: Context Export CLI with shared assembly engine"
PR body: "Adds top-level `aether context` command for clipboard-ready intelligence
export. Refactors sir_context.rs into shared assembly core + renderers. Supports
file targets, symbol targets, overview mode, budget tiers, layer selection, and
markdown/JSON output. sir-context retained as compatibility alias. Graceful
degradation when SurrealKV locked or index unavailable."
```

---

## Stage R.2 — SIR-Guided File Slicing

**Codename:** Scalpel
**Depends on:** R.1 (Context Export)
**New crates:** None
**Modified crates:** `aetherd` (context assembly), references `symbols` table parse spans

### Purpose

Replace whole-file source blocks in context output with symbol-guided slices derived from existing parse spans in the `symbols` table. This can reduce source-code token usage by 50–80% for large files, freeing budget for more intelligence layers.

### What to Build

#### File Slicer

```rust
pub struct FileSlice {
    pub file: PathBuf,
    pub sections: Vec<SliceSection>,
    pub tokens_saved: usize,
}

pub struct SliceSection {
    pub symbol_id: String,
    pub symbol_name: String,
    pub start_line: usize,
    pub end_line: usize,
    pub source: String,
    pub context_lines_before: usize,
    pub context_lines_after: usize,
}
```

#### Selection Logic

Given target symbols and their graph neighborhood:

1. **Primary targets (depth 0):** Full symbol definition (function body, struct + impl block)
2. **Direct callers/callees (depth 1):** Signature + first doc comment line. Skip body unless budget allows.
3. **Depth 2+ neighbors:** Signature only (one line).
4. **Adjacent merge:** If two selected symbols are within 5 lines in the same file, merge into a single range.
5. **Elision markers:** Between non-adjacent ranges, insert `// ... (N lines omitted) ...`
6. **Small file fallback:** Files under 50 lines include whole file (not worth slicing).

#### Integration with R.1

The assembly core's "target source" layer (Priority 1) switches from whole-file to sliced output. Same budget allocation, but 30% of budget now covers more relevant code.

### Pass Criteria

1. Single-symbol extraction returns correct byte range from parse spans.
2. Adjacent symbols within 5 lines merge into single range.
3. Elision markers show correct omitted line counts.
4. Context lines (before/after) configurable via `--context-lines` flag, default 3.
5. Files under 50 lines return whole file.
6. Token savings reported in budget footer.
7. `cargo fmt`, `cargo clippy -p aetherd`, `cargo test -p aetherd` pass.

### Exact Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_repo_the_prompter.md — focus on Stage R.2.

PREFLIGHT:
1) git status --porcelain (must be clean)
2) git pull --ff-only
3) git worktree add -b feature/phase-repo-stage-r2-file-slicing /home/rephu/phase-repo-r2
4) cd /home/rephu/phase-repo-r2

IMPLEMENTATION:

5) Add FileSlice and SliceSection structs to the context assembly engine.

6) Implement slice_file_for_symbols():
   - Query symbol metadata (start_line, end_line) from symbols table
   - Depth 0 targets: full body
   - Depth 1: signature + doc comment
   - Depth 2+: signature only
   - Merge adjacent ranges within 5 lines
   - Insert elision markers between non-adjacent ranges
   - Files under 50 lines: return whole file

7) Update R.1's assembly core: replace whole-file source inclusion with sliced output.

8) Add --context-lines flag (default 3).

9) Tests:
   - File with 3 functions, request 1 → only that function extracted
   - Two adjacent functions → merged into single range
   - File under 50 lines → whole file returned
   - Elision markers show correct line counts
   - Token savings calculated correctly

10) cargo fmt --all --check && cargo clippy -p aetherd -- -D warnings && cargo test -p aetherd

11) Commit: "Add SIR-guided file slicing for token-efficient context export"

PR title: "Phase Repo R.2: SIR-guided file slicing"
PR body: "Replaces whole-file source inclusion with symbol-guided slices using
existing parse spans. Merges adjacent ranges, inserts elision markers, falls
back to whole file for small files. Reduces source token usage 50-80% for
large files like aether-store/src/lib.rs."
```

---

## Stage R.3 — Prompt Preset Library

**Codename:** Playbook
**Depends on:** R.1 (Context Export)
**New crates:** None
**Modified crates:** `aetherd` (preset management subcommands)

### Purpose

Save named, reusable context configurations as presets. Instead of remembering `aether context --include sir,graph,coupling --depth 3 --budget 64000`, save it as `aether context --preset deep`.

### Preset Schema (TOML)

```toml
# .aether/presets/refactor-plan.toml
[preset]
name = "refactor-plan"
description = "Full intelligence dump for planning a refactor"

[context]
budget = 96000
depth = 3
include = ["sir", "source", "graph", "coupling", "health", "drift", "memory", "tests"]
format = "markdown"
context_lines = 5

[task_template]
template = "Plan a refactor of {target}. Consider coupling, health warnings, and test coverage gaps."
```

### CLI Commands

```bash
aether context --preset deep crates/aether-mcp/src/lib.rs    # Use a preset
aether preset list                                             # List all presets
aether preset show deep                                        # Show preset details
aether preset create my-preset                                 # Create interactively
aether preset delete my-preset                                 # Remove user preset
```

### Built-in Presets (embedded in binary)

| Name | Budget | Depth | Layers | Use Case |
|------|--------|-------|--------|----------|
| `quick` | 8K | 1 | sir, source | Quick question about a symbol |
| `review` | 32K | 2 | sir, source, graph, coupling, health, tests | Code review context |
| `deep` | 64K | 3 | all layers | Deep analysis or refactor planning |
| `overview` | 16K | 0 | sir, health, drift | Project-level health check |

User presets in `.aether/presets/` override built-ins with the same name. CLI flags always override preset values.

### Pass Criteria

1. `aether context --preset <n>` applies all preset settings.
2. CLI flags override preset values.
3. `aether preset list` shows built-in + user presets with descriptions.
4. Task template variable substitution works (`{target}` replaced with actual file/symbol).
5. User presets override built-ins with same name.
6. Invalid preset TOML produces clear error message.
7. `cargo fmt`, `cargo clippy -p aetherd`, `cargo test -p aetherd` pass.

### Exact Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_repo_the_prompter.md — focus on Stage R.3.

PREFLIGHT:
1) git status --porcelain (must be clean)
2) git pull --ff-only
3) git worktree add -b feature/phase-repo-stage-r3-presets /home/rephu/phase-repo-r3
4) cd /home/rephu/phase-repo-r3

IMPLEMENTATION:

5) Define PresetConfig struct matching TOML schema.

6) Implement preset loading:
   - Built-in: quick, review, deep, overview (embedded in binary)
   - User: scan .aether/presets/*.toml
   - User overrides built-in if same name

7) Add `aether preset` subcommand group: list, show, create, delete.

8) Integrate with `aether context`: --preset loads config, explicit CLI flags override.

9) Task template: replace {target} with file/symbol arguments.

10) Tests:
    - Preset loading from TOML
    - CLI flags override preset values
    - User preset overrides built-in
    - Template substitution
    - Invalid TOML error handling

11) cargo fmt --all --check && cargo clippy -p aetherd -- -D warnings && cargo test -p aetherd

12) Commit: "Add prompt preset library for reusable context configurations"

PR title: "Phase Repo R.3: Prompt preset library"
PR body: "Adds .aether/presets/ TOML-based presets with 4 built-in defaults
(quick/review/deep/overview). CLI flags override preset values. Task templates
support {target} variable substitution."
```

---

## Stage R.4 — Multi-Format Output

**Codename:** Translator
**Depends on:** R.1 (Context Export)
**New crates:** None
**Modified crates:** `aetherd` (output formatters)

### Purpose

Formalize a `Formatter` trait and add XML and compact renderers on top of the same `ExportDocument` that R.1 already produces.

### Formats

| Format | Flag | Use Case |
|--------|------|----------|
| `markdown` | `--format markdown` (default) | ChatGPT, Claude web, Gemini |
| `json` | `--format json` | Programmatic consumption, piping |
| `xml` | `--format xml` | Claude API prompts, structured context |
| `compact` | `--format compact` | Maximum density, small budgets |

### XML Format Example

```xml
<aether_context workspace="/home/rephu/projects/aether" generated="2026-03-15T14:30:00Z">
  <overview symbols="3748" sir_coverage="100%" health="42" />
  <target file="crates/aether-store/src/graph.rs">
    <symbol name="GraphStore" kind="trait" health="82">
      <sir>
        <intent>Defines the abstract interface for persisting and querying dependency edges</intent>
        <edge_cases>Empty graph returns empty vecs, duplicate edges idempotent</edge_cases>
        <dependencies>SymbolId, EdgeKind, GraphEdge</dependencies>
      </sir>
    </symbol>
  </target>
  <graph depth="2">
    <edge source="build_dependency_graph" target="GraphStore::upsert_edges" type="CALLS" />
  </graph>
  <coupling>
    <pair file_a="graph.rs" file_b="edge.rs" score="0.91" />
  </coupling>
  <warnings>
    <test_gap symbol="GraphStore" sir_edge_cases="4" test_guards="2" />
  </warnings>
</aether_context>
```

### Compact Format Example

```
=== AETHER Context: crates/aether-store/src/graph.rs ===
Budget: 8K | Symbols: 12 | Health: 82/100

[GraphStore] Trait
  Intent: Abstract interface for dependency edge persistence and querying
  Edges: Empty graph → empty vecs, duplicate edges → idempotent
  Deps: SymbolId, EdgeKind, GraphEdge
  Callers: build_dependency_graph (crates/aetherd/src/indexer.rs)

[SurrealGraphStore] Struct
  Intent: SurrealDB-backed GraphStore with Record References
  ...
```

### Pass Criteria

1. XML output is valid XML (parseable by standard parsers).
2. JSON schema unchanged from R.1.
3. Compact output is denser than markdown for same content.
4. All formats include budget usage metadata.
5. `cargo fmt`, `cargo clippy -p aetherd`, `cargo test -p aetherd` pass.

### Exact Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_repo_the_prompter.md — focus on Stage R.4.

PREFLIGHT:
1) git status --porcelain (must be clean)
2) git pull --ff-only
3) git worktree add -b feature/phase-repo-stage-r4-formats /home/rephu/phase-repo-r4
4) cd /home/rephu/phase-repo-r4

IMPLEMENTATION:

5) Define Formatter trait: fn format(doc: &ExportDocument) -> String

6) Extract existing markdown and JSON renderers from R.1 into trait impls.

7) Add XmlFormatter: structured <aether_context> document with nested elements.

8) Add CompactFormatter: dense single-line-per-symbol format.

9) Wire --format xml|compact to new formatters.

10) Tests:
    - XML parses as valid XML
    - JSON unchanged from R.1
    - Compact uses fewer tokens than markdown for same content
    - All formats include budget metadata

11) cargo fmt --all --check && cargo clippy -p aetherd -- -D warnings && cargo test -p aetherd

12) Commit: "Add XML and compact formatters for context export"

PR title: "Phase Repo R.4: XML and compact output formats"
PR body: "Formalizes Formatter trait. Adds XML (structured <aether_context> tags
for Claude API prompts) and compact (maximum density for small budgets) output
formats alongside existing markdown and JSON."
```

---

## Stage R.5 — Interactive Context Builder (Dashboard)

**Codename:** Workbench
**Depends on:** R.1 (Context Export), R.2 (File Slicing) recommended
**New crates:** None
**Modified crates:** `aether-dashboard` (new page + API endpoint)
**Scope:** Dashboard-only. No TUI in Phase Repo.

### Purpose

Add a dashboard page where developers can interactively build context: browse the file tree, select symbols, see live token budget consumption, toggle intelligence layers, and copy the result. This reuses the existing HTMX + D3 dashboard infrastructure from Phase 7.6/7.9 and the existing `SharedState` + symbol catalog.

### What to Build

#### API Endpoint

```
POST /api/v1/context/build
{
  "targets": ["crates/aether-store/src/graph.rs"],
  "budget": 32000,
  "depth": 2,
  "layers": { "sir": true, "source": true, "graph": true, ... },
  "format": "markdown",
  "task": "review error handling"
}

→ Response:
{
  "content": "# AETHER Context: ...",
  "budget_usage": {
    "total": 32000,
    "used": 24180,
    "by_layer": { "sir": 3600, "source": 7200, ... }
  }
}
```

#### Dashboard Page: `/dashboard/context-builder`

```
┌─────────────────────────────────────────────────────────────────┐
│ Context Builder                    Budget: [====----] 24K / 32K │
├──────────────────┬──────────────────────────────────────────────┤
│ File Tree        │ Preview                                      │
│ ☐ crates/        │                                              │
│   ☑ aether-store │ # AETHER Context: .../graph.rs               │
│     ☑ graph.rs   │ ## GraphStore (Trait)                         │
│     ☐ edge.rs    │ **Intent:** Defines the abstract interface... │
│   ☐ aether-mcp   │ ...                                          │
│                  │                                              │
│ ──────────────── │                                              │
│ Layers           │                                              │
│ ☑ SIR            │                                              │
│ ☑ Source Code    │                                              │
│ ☑ Graph          │                                              │
│ ☐ Coupling       │                                              │
│ ☑ Health         │                                              │
│ ──────────────── │                                              │
│ Task: [________] │                                              │
│ Depth: [2   ▼]  │                                              │
│ Preset: [review] │                                              │
│ [Copy] [Export]  │                                              │
├──────────────────┴──────────────────────────────────────────────┤
│ SIR 3.6K | Source 7.2K | Graph 4.1K | Health 1.2K | = 24.2K    │
└─────────────────────────────────────────────────────────────────┘
```

**Interactions:**
- Check/uncheck files → HTMX partial swap updates preview + budget bar
- Toggle layers → preview updates
- Copy button → `navigator.clipboard.writeText()`
- Export button → download as file
- Preset dropdown → loads R.3 presets
- Task field → biases context selection

### Pass Criteria

1. Dashboard page renders file tree with checkboxes.
2. Selecting files updates token budget display via HTMX.
3. Layer toggles update preview.
4. Copy button copies formatted context to clipboard.
5. Preset dropdown loads R.3 presets.
6. Budget breakdown shows per-layer token usage.
7. `cargo fmt`, `cargo clippy -p aetherd --features dashboard`, `cargo test -p aetherd` pass.

### Exact Codex Prompt

```text
CRITICAL BUILD SETTINGS — use these for ALL cargo commands in this session:
- export CARGO_TARGET_DIR=/home/rephu/aether-target
- export CARGO_BUILD_JOBS=2
- export PROTOC=$(which protoc)
- export RUSTC_WRAPPER=sccache
- export TMPDIR=/home/rephu/aether-target/tmp
- mkdir -p $TMPDIR

You are working in the repo root of https://github.com/rephug/aether.

Read docs/roadmap/phase_repo_the_prompter.md — focus on Stage R.5.

PREFLIGHT:
1) git status --porcelain (must be clean)
2) git pull --ff-only
3) git worktree add -b feature/phase-repo-stage-r5-builder /home/rephu/phase-repo-r5
4) cd /home/rephu/phase-repo-r5

IMPLEMENTATION:

5) Add POST /api/v1/context/build to aether-dashboard:
   - Accept ExportContextRequest JSON body
   - Call R.1's assembly core
   - Return formatted content + budget usage

6) Add /dashboard/context-builder page:
   - File tree with checkboxes (HTMX from existing file list)
   - Layer toggles, task input, depth selector, format selector
   - Preview panel (HTMX partial swap on change)
   - Live budget bar + per-layer breakdown footer
   - Copy button (navigator.clipboard.writeText)
   - Export/download button
   - Preset dropdown (from R.3)

7) Add sidebar link for Context Builder page.

8) Tests:
   - API endpoint returns expected JSON schema
   - Budget usage sums correctly

9) cargo fmt --all --check && cargo clippy -p aetherd --features dashboard -- -D warnings && cargo test -p aetherd

10) Commit: "Add interactive context builder dashboard page"

PR title: "Phase Repo R.5: Interactive context builder"
PR body: "Adds /dashboard/context-builder page with file tree selection, live
token budget, layer toggles, preset loading, and clipboard/export. Uses HTMX
partial swaps and existing SharedState. Dashboard-only, no TUI."
```

---

## Estimated Effort

| Stage | Codex Runs | Calendar Time | Priority |
|-------|------------|---------------|----------|
| R.1 Context Export CLI | 1–2 | 3–5 days | **Highest — do first** |
| R.2 SIR-Guided File Slicing | 1 | 1–2 days | High |
| R.3 Prompt Preset Library | 1–2 | 2–3 days | Medium |
| R.4 Multi-Format Output | 1 | 1–2 days | Medium |
| R.5 Interactive Context Builder | 1–2 | 3–5 days | Lower |
| **Total** | **5–8** | **~2–3 weeks** | |

---

## Sequencing Relative to Other Phases

```
Phase 8 remaining (health inversion + boundary leaker + 8.14b)
    ↓
Phase 10 (Conductor — batch, continuous, agent hooks)
    ↓
Phase Repo R.1–R.5 ← slots after 10.3 or during 10.x
    ↓
Phase 9 (Beacon — Tauri app)
```

**Phase 10.3 overlap note:** Phase 10.3 defines `sir context` for agent-facing context assembly. Phase Repo R.1 generalizes that same engine for human-facing export. If 10.3 ships first, R.1 refactors its engine. If Phase Repo ships first, 10.3 reuses R.1's assembly core. Either ordering works.

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| SurrealKV lock contention blocks context command | High | High | All SurrealDB access is best-effort. Coupling layer omitted with notice. All other layers use SQLite (concurrent readers). |
| Token estimation inaccuracy | Low | Low | 4-chars-per-token heuristic is conservative. Users adjust budget if needed. |
| sir-context compatibility regression | Medium | Medium | Existing sir-context tests must pass unchanged after refactor. |
| Health/drift data incomplete for some symbols | Medium | Low | Layers degrade gracefully. Missing data → section omitted with notice. |

---

## What Phase Repo Does NOT Do

- **Does not replace MCP tools.** MCP remains the primary interface for connected agents.
- **Does not add new intelligence.** Every layer already exists. Phase Repo is a read-only formatter.
- **Does not do agent orchestration.** Context assembly is deterministic (graph traversal + priority ranking), not agent-driven discovery.
- **Does not write files.** Pure read. No apply mode.
- **Does not include a TUI.** R.5 is dashboard-only. TUI could be a future addition.
- **Does not add a new crate.** Everything lives in `aetherd` and `aether-dashboard`.
