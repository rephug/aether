# Phase Repo — The Prompter

**Codename:** The Prompter
**Thesis:** AETHER already knows your codebase better than any context-engineering tool. This phase makes that intelligence *portable* — exportable to any AI chat, any agent, any clipboard — so developers can carry AETHER's understanding into any conversation, not just MCP-connected ones.

**Inspiration:** RepoPrompt demonstrated that developers want structured context assembly for AI conversations. AETHER already has deeper intelligence (SIR, graph, coupling, drift, health) but currently only exposes it through MCP tools and CLI queries. Phase Repo bridges the gap: AETHER's intelligence, formatted for human-pasted or agent-consumed context windows.

**One-sentence summary:** "Your codebase intelligence, anywhere you need it."

---

## Why This Phase, Why Now

1. **Immediate daily value.** Robert's workflow involves consulting Claude, Gemini Deep Think, and ChatGPT — none of which are MCP-connected. Today that means manually assembling context. Phase Repo automates this.

2. **Low implementation cost.** Every feature builds on existing infrastructure (context assembly engine, SIR store, graph queries, token budgeting). No new databases, no new protocols, no new crates. Mostly new CLI commands and formatters on top of existing query paths.

3. **Amplifies existing investment.** Phase 8 built a world-class understanding engine. Phase 10 makes it autonomous. Phase Repo makes it *accessible* — the missing bridge between intelligence and consumption.

4. **Competitive moat.** RepoPrompt's code maps are shallow structural overviews. AETHER's context export includes semantic intent, dependency graphs, coupling data, health scores, and drift warnings. No other tool can produce context this rich.

---

## Stage Plan

| Stage | Name | Scope | Codex Runs | Dependencies |
|-------|------|-------|------------|--------------|
| R.1 | Context Export CLI | `aether context` command — clipboard-ready, token-budgeted context assembly | 1–2 | Phase 8 complete (SIR, graph, health) |
| R.2 | SIR-Guided File Slicing | Symbol-range extraction for token-efficient file inclusion | 1 | R.1 |
| R.3 | Prompt Preset Library | Named, reusable prompt templates with variable substitution | 1–2 | R.1 |
| R.4 | Multi-Format Output | Markdown, XML, JSON, and agent-native output formats | 1 | R.1 |
| R.5 | Interactive Context Builder | TUI/dashboard-based context assembly with live token counting | 1–2 | R.1, R.2 |

### Dependency Chain

```
R.1 (Context Export) ──► R.2 (File Slicing)
        │                      │
        ├──► R.3 (Presets)     │
        │                      │
        ├──► R.4 (Formats)     │
        │                      │
        └──► R.5 (Interactive) ◄┘
```

R.2, R.3, and R.4 are independent after R.1. R.5 benefits from R.2 but can start without it.

**Priority order for implementation:** R.1 → R.2 → R.3 → R.4 → R.5

---

## Decisions to Lock for Phase Repo

| # | Decision | Resolution | Rationale |
|---|----------|------------|-----------|
| 97 | Context export lives in existing `aetherd` binary | CLI subcommand, not a new crate. Uses existing `SharedState`. | No new binary, no new dependencies. Context assembly already exists in `aether-generate` (Phase 8.1 spec). The export command is a read-only formatter on top of it. |
| 98 | Default token budget for export: 32K tokens | Same as Phase 8 context assembly (Decision #48). Configurable via `--budget` flag. | 32K fits in every major model's context window. Users can raise for 128K+ models. |
| 99 | Default output: markdown to stdout | `aether context` prints to stdout. Pipe to `pbcopy`/`xclip`/`xsel` for clipboard. No built-in clipboard dependency. | Cross-platform without linking to platform clipboard libraries. Shell piping is universal. Adding `| pbcopy` or `| xclip -selection clipboard` is one extra shell token. |
| 100 | Presets stored in `.aether/presets/` as TOML files | One file per preset. Human-editable. Version-controllable. | TOML is already the config format. No new parser needed. Users can share presets by committing the directory. |
| 101 | File slicing granularity: symbol-level | Slice at symbol boundaries (function, struct, impl block, etc.) using existing tree-sitter span data. Not line-level or AST-node-level. | Symbol-level matches SIR granularity. AETHER already knows every symbol's byte range from parsing. Finer granularity adds complexity without proportional value. |
| 102 | Context priority tiers (9 tiers) | Same priority ranking as Phase 10.3 `sir context` (if implemented first) or Phase 8.1 budget system. Tiers: (1) target file slices, (2) target SIRs, (3) immediate caller/callee SIRs, (4) test intents, (5) coupling data, (6) project memory, (7) health warnings, (8) drift alerts, (9) broader graph neighborhood. | Consistent with existing context assembly design. Health warnings and drift alerts are new additions that leverage Phase 8's unique intelligence. |

---

## Stage R.1 — Context Export CLI

**Codename:** Courier
**Depends on:** Phase 8 complete (SIR store, graph, health scores)
**New crates:** None
**Modified crates:** `aetherd` (new subcommand), possibly `aether-generate` (reuse context assembly)

### Purpose

Add an `aether context` CLI command that assembles AETHER's intelligence into a single, token-budgeted document suitable for pasting into any AI chat interface. This is the "clipboard-ready prompt export" — the single most immediately useful feature from RepoPrompt's playbook, except AETHER's version includes semantic intelligence that RepoPrompt can't produce.

### What to Build

#### CLI Interface

```bash
# Basic: context for a specific file
aether context src/payments/processor.rs

# Context for a specific symbol
aether context --symbol validate_payment_amount --file src/payments/processor.rs

# Context for multiple targets
aether context src/payments/processor.rs src/payments/validator.rs

# With budget control
aether context src/payments/processor.rs --budget 64000

# With depth control (how many hops in the dependency graph)
aether context src/payments/processor.rs --depth 2

# Include specific intelligence layers
aether context src/payments/processor.rs --include sir,graph,coupling,health,drift

# Exclude layers
aether context src/payments/processor.rs --exclude drift,memory

# Output format (default: markdown)
aether context src/payments/processor.rs --format markdown
aether context src/payments/processor.rs --format xml
aether context src/payments/processor.rs --format json

# Pipe to clipboard (platform-dependent, user's responsibility)
aether context src/payments/processor.rs | xclip -selection clipboard
aether context src/payments/processor.rs | pbcopy  # macOS

# Task-oriented: provide a task description for smarter context selection
aether context --task "add rate limiting to the API endpoint" src/api/routes.rs

# Overview mode: project-level summary without targeting specific files
aether context --overview
aether context --overview --budget 16000
```

#### Output Structure (Markdown Format)

```markdown
# AETHER Context: src/payments/processor.rs
Generated: 2026-03-15T14:30:00Z | Budget: 32,000 tokens | Used: 28,412 tokens

## Project Overview
- **Workspace:** /home/rephu/projects/aether
- **Total Symbols:** 3,748 | **SIR Coverage:** 100%
- **Health Score:** 42/100 (Watch)

## Target File: src/payments/processor.rs
### Symbols (7)

#### `validate_payment_amount` (Function)
**Intent:** Validates that a payment amount is positive, non-zero, and within
the account's available balance. Returns a typed error for each failure mode.
**Edge Cases:** Zero amount → InvalidAmount, negative → InvalidAmount,
exceeds balance → InsufficientFunds, overflow → AmountOverflow
**Error Handling:** Returns Result<ValidatedAmount, PaymentError>
**Dependencies:** AccountBalance, PaymentError, ValidatedAmount
**Health:** 78/100 | **Drift:** None

#### `process_payment` (Function)
**Intent:** Orchestrates the full payment flow: validate → authorize → capture → record.
...

### File Source (Relevant Sections)
```rust
// Lines 45-78: validate_payment_amount
pub fn validate_payment_amount(
    amount: Decimal,
    balance: &AccountBalance,
) -> Result<ValidatedAmount, PaymentError> {
    ...
}
```

## Dependency Neighborhood (1-hop)

### Callers of target symbols
| Symbol | File | Relationship |
|--------|------|-------------|
| `handle_payment_request` | src/api/routes.rs | CALLS → validate_payment_amount |
| `batch_processor` | src/jobs/payments.rs | CALLS → process_payment |

### Callees from target symbols
| Symbol | File | Relationship |
|--------|------|-------------|
| `AccountBalance::available` | src/models/account.rs | validate_payment_amount → CALLS |
| `authorize_payment` | src/payments/gateway.rs | process_payment → CALLS |

### Neighbor SIRs (summarized)
**`handle_payment_request`** — Parses HTTP request body into PaymentRequest,
validates via validate_payment_amount, returns 200 with receipt or 4xx with error detail.
...

## Coupling Data
| File | Coupling Score | Signal |
|------|---------------|--------|
| src/payments/validator.rs | 0.87 | Co-change in 12/15 commits |
| src/api/routes.rs | 0.63 | Shared caller pattern |

## Health Warnings
- **Boundary Leaker:** process_payment calls into 2 communities (payments + notifications)
- **Test Coverage Gap:** validate_payment_amount has 7 SIR edge cases but only 3 test guards

## Active Drift
(None detected for target symbols)

## Relevant Project Memory
- "Chose rust_decimal over f64 for money — precision requirement from Payment Gateway v2 spec" (2026-02-14)
```

#### Context Assembly Engine

```rust
/// Assembled context for export, built from AETHER's intelligence layers.
pub struct ExportContext {
    /// Target files/symbols the user requested
    pub targets: Vec<ExportTarget>,

    /// Optional task description for smarter context selection
    pub task: Option<String>,

    /// Token budget
    pub budget: TokenBudget,

    /// Which intelligence layers to include
    pub layers: LayerSelection,

    /// Graph traversal depth
    pub depth: u8,
}

pub struct LayerSelection {
    pub sir: bool,           // SIR annotations for target + neighbor symbols
    pub source: bool,        // Actual source code (file slices in R.2, whole files in R.1)
    pub graph: bool,         // Dependency neighborhood (callers, callees)
    pub coupling: bool,      // Co-change coupling data
    pub health: bool,        // Health warnings and scores
    pub drift: bool,         // Active drift alerts
    pub memory: bool,        // Relevant project memory notes
    pub tests: bool,         // Test intents for target symbols
}

impl Default for LayerSelection {
    fn default() -> Self {
        Self {
            sir: true,
            source: true,
            graph: true,
            coupling: true,
            health: true,
            drift: true,
            memory: true,
            tests: true,
        }
    }
}
```

#### Token Budget Allocation

Reuses the priority-ranked budget system from Phase 8.1 / Phase 10.3:

| Priority | Layer | Default % of Budget | Rationale |
|----------|-------|-------------------|-----------|
| 1 | Target file source (sliced in R.2) | 30% | The AI needs to see the actual code |
| 2 | Target symbol SIRs | 15% | Semantic understanding of what the code does |
| 3 | Immediate neighbor SIRs | 15% | Context for how target fits into the system |
| 4 | Test intents | 10% | What's tested, what's not |
| 5 | Coupling data | 8% | What files change together |
| 6 | Project memory | 7% | Architectural decisions, past context |
| 7 | Health warnings | 5% | Known issues with these symbols |
| 8 | Drift alerts | 5% | Where code and intent have diverged |
| 9 | Broader graph | 5% | Extended dependency neighborhood |

When budget is exhausted, lower-priority layers are truncated or omitted. The output includes a "Budget Usage" section showing what was included vs. truncated.

### Pass Criteria

1. `aether context <file>` produces markdown output with SIR summaries, dependency neighborhood, coupling data, health warnings.
2. `--budget` flag correctly limits output size (measured in estimated tokens).
3. `--include` and `--exclude` flags control which intelligence layers appear.
4. `--depth` controls graph traversal depth.
5. `--format markdown` (default) produces clean, pasteable markdown.
6. `--overview` mode produces project-level summary without file targets.
7. `--task` mode biases context selection toward task-relevant symbols (via semantic search).
8. Output includes budget usage summary (included/truncated/omitted layers).
9. Empty workspace (no SIRs) gracefully degrades to file-only output.
10. `cargo fmt --all --check`, `cargo clippy`, `cargo test` pass.

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

1) Ensure working tree is clean. If not, stop and report dirty files.
2) git worktree add -b feature/phase-repo-stage-r1-context-export /home/rephu/phase-repo-r1
3) cd /home/rephu/phase-repo-r1

4) In crates/aetherd — add `context` subcommand:
   - Parse CLI args: file targets, --symbol, --budget (default 32000), --depth (default 2),
     --include, --exclude, --format (markdown|xml|json), --task, --overview
   - Build ExportContext from args
   - Call context assembly engine
   - Format and print to stdout

5) Context assembly engine (in aetherd or aether-generate):
   - Given targets + budget + layers + depth:
     a. Resolve target files/symbols via existing store queries
     b. Load SIRs for target symbols
     c. Traverse dependency graph to --depth, load neighbor SIRs
     d. Load coupling data for target files
     e. Load health scores and warnings for target symbols
     f. Load drift data for target symbols
     g. Load relevant project memory (semantic search with task or file names)
     h. Load test intents for target symbols
   - Apply token budget: estimate tokens per layer, truncate from priority 9 upward
   - Return assembled context struct

6) Markdown formatter:
   - Header with metadata (workspace, timestamp, budget used)
   - Project overview section
   - Per-target-file sections with symbol SIRs
   - Source code sections (whole file for now — R.2 adds slicing)
   - Dependency neighborhood tables
   - Neighbor SIR summaries
   - Coupling data table
   - Health warnings list
   - Drift alerts list
   - Project memory notes
   - Budget usage summary footer

7) Token estimation:
   - Use simple heuristic: 1 token ≈ 4 characters (conservative)
   - Or reuse tiktoken-rs if already in deps, otherwise the heuristic is fine

8) Add tests:
   - Mock store with known symbols, SIRs, edges → verify output contains expected sections
   - Budget truncation: set budget to 1000 tokens → verify lower-priority layers omitted
   - --include sir,graph → verify only those layers present
   - --exclude drift → verify drift section absent
   - --overview mode → verify project summary without file sections
   - Empty workspace → graceful degradation

9) Run:
   - cargo fmt --all --check
   - cargo clippy -p aetherd -- -D warnings
   - cargo test -p aetherd

10) Commit: "Add aether context CLI for clipboard-ready intelligence export"
```

---

## Stage R.2 — SIR-Guided File Slicing

**Codename:** Scalpel
**Depends on:** R.1 (Context Export)
**New crates:** None
**Modified crates:** `aetherd` (context assembly), `aether-parse` (symbol byte ranges)

### Purpose

Instead of including entire files in context output, extract only the symbol definitions that are relevant to the query. Uses AETHER's existing tree-sitter parse data (which already knows every symbol's byte range) to slice files at symbol boundaries. This can reduce source-code token usage by 50–80% for large files, freeing budget for more intelligence layers.

### What to Build

#### File Slicer

```rust
/// Extract relevant symbol ranges from a file.
pub struct FileSlice {
    /// Source file path
    pub file: PathBuf,

    /// Extracted ranges with context
    pub sections: Vec<SliceSection>,

    /// Total tokens saved vs including the whole file
    pub tokens_saved: usize,
}

pub struct SliceSection {
    /// Symbol ID this section belongs to
    pub symbol_id: String,

    /// Symbol name for labeling
    pub symbol_name: String,

    /// Start line (1-indexed)
    pub start_line: usize,

    /// End line (1-indexed)
    pub end_line: usize,

    /// The actual source code
    pub source: String,

    /// Lines of surrounding context (configurable, default 3)
    pub context_lines_before: usize,
    pub context_lines_after: usize,
}
```

#### Selection Logic

Given a set of target symbols and their graph neighborhood:

1. **Primary targets:** Always include the full symbol definition (function body, struct + impl block, etc.)
2. **Direct callers/callees (1-hop):** Include signature + first doc comment line. Skip body unless budget allows.
3. **2-hop neighbors:** Include signature only (one line).
4. **Merging adjacent ranges:** If two selected symbols are within 5 lines of each other in the same file, merge into a single range to preserve context flow.
5. **Elision markers:** Between non-adjacent ranges, insert `// ... (N lines omitted) ...` markers.

#### Integration with R.1

The context assembly engine's "Target file source" layer (Priority 1) switches from "whole file" to "sliced file" when R.2 is available. The budget allocation remains the same, but the same 30% budget now covers more relevant code.

### Pass Criteria

1. Slicing correctly extracts symbol byte ranges from tree-sitter parse data.
2. Adjacent symbols within 5 lines merge into single range.
3. Elision markers show correct omitted line counts.
4. Context lines (before/after) configurable, default 3.
5. Token savings reported in output.
6. Whole-file fallback when file has fewer than 50 lines (not worth slicing).
7. `cargo fmt`, `cargo clippy`, `cargo test` pass.

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
Focus on Stage R.2 — SIR-Guided File Slicing.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) git worktree add -b feature/phase-repo-stage-r2-file-slicing /home/rephu/phase-repo-r2
3) cd /home/rephu/phase-repo-r2

4) In the context assembly engine (where R.1 builds ExportContext):
   - Add FileSlice struct and SliceSection struct
   - Implement slice_file_for_symbols(file_path, symbol_ids, depth_map, context_lines) -> FileSlice
   - Query existing symbol metadata (start_line, end_line from `symbols` table or tree-sitter spans)
   - For primary targets (depth 0): include full symbol body
   - For depth 1: include signature + doc comment
   - For depth 2+: include signature only (one line)
   - Merge adjacent ranges within 5 lines
   - Insert elision markers between non-adjacent ranges
   - Skip slicing for files under 50 lines (include whole file)

5) Update R.1's markdown formatter to use sliced output:
   - Replace whole-file inclusion with sliced sections
   - Show "N lines omitted" markers
   - Show token savings in budget summary

6) Add --context-lines flag (default 3) for controlling surrounding context

7) Add tests:
   - File with 3 functions, request 1 → only that function extracted
   - Two adjacent functions → merged into single range
   - File under 50 lines → whole file returned
   - Token savings calculated correctly
   - Elision markers show correct line counts

8) Run:
   - cargo fmt --all --check
   - cargo clippy -p aetherd -- -D warnings
   - cargo test -p aetherd

9) Commit: "Add SIR-guided file slicing for token-efficient context export"
```

---

## Stage R.3 — Prompt Preset Library

**Codename:** Playbook
**Depends on:** R.1 (Context Export)
**New crates:** None
**Modified crates:** `aetherd` (preset management subcommands)

### Purpose

Save named, reusable context configurations as presets. Instead of remembering `aether context --include sir,graph,coupling --depth 3 --budget 64000`, save it as `aether context --preset deep-review`. Presets can also include task templates with variable placeholders.

### What to Build

#### Preset Schema (TOML)

```toml
# .aether/presets/deep-review.toml
[preset]
name = "deep-review"
description = "Deep code review context with full graph traversal"

[context]
budget = 64000
depth = 3
include = ["sir", "source", "graph", "coupling", "health", "drift", "tests"]
format = "markdown"
context_lines = 5

[task_template]
# Optional: pre-fill --task with a template. {target} is replaced with the file/symbol arg.
template = "Review {target} for correctness, edge case handling, and error propagation. Flag any coupling concerns."
```

```toml
# .aether/presets/quick-explain.toml
[preset]
name = "quick-explain"
description = "Lightweight context for quick questions about a symbol"

[context]
budget = 8000
depth = 1
include = ["sir", "source"]
format = "markdown"
context_lines = 0
```

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
template = "Plan a refactor of {target}. Consider coupling, health warnings, and test coverage gaps. Propose a migration order based on dependency structure."
```

#### CLI Commands

```bash
# Use a preset
aether context --preset deep-review src/payments/processor.rs

# List available presets
aether preset list

# Show preset details
aether preset show deep-review

# Create a preset interactively (writes TOML file)
aether preset create my-preset

# Delete a preset
aether preset delete my-preset
```

#### Built-in Presets

Ship 4 built-in presets that are always available (stored in binary, not in `.aether/presets/`):

| Name | Budget | Depth | Layers | Use Case |
|------|--------|-------|--------|----------|
| `quick` | 8K | 1 | sir, source | Quick question about a symbol |
| `review` | 32K | 2 | sir, source, graph, coupling, health, tests | Code review context |
| `deep` | 64K | 3 | all layers | Deep analysis or refactor planning |
| `overview` | 16K | 0 | sir, health, drift | Project-level health check |

User presets in `.aether/presets/` override built-ins with the same name.

### Pass Criteria

1. `aether context --preset <name>` applies all preset settings.
2. CLI flags override preset values (e.g., `--preset deep --budget 128000` uses deep preset but overrides budget).
3. `aether preset list` shows built-in + user presets with descriptions.
4. Task template variable substitution works ({target} replaced with actual file/symbol).
5. User presets in `.aether/presets/` override built-ins.
6. Invalid preset TOML produces clear error message.
7. `cargo fmt`, `cargo clippy`, `cargo test` pass.

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
Focus on Stage R.3 — Prompt Preset Library.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) git worktree add -b feature/phase-repo-stage-r3-presets /home/rephu/phase-repo-r3
3) cd /home/rephu/phase-repo-r3

4) Define Preset struct matching the TOML schema:
   - PresetConfig { name, description, context: ContextSettings, task_template: Option }
   - ContextSettings { budget, depth, include, format, context_lines }

5) Implement preset loading:
   - Built-in presets: quick, review, deep, overview (embedded in binary)
   - User presets: scan .aether/presets/*.toml
   - User overrides built-in if same name

6) Add `aether preset` subcommand group:
   - `list` — table of name, description, budget, depth
   - `show <name>` — full preset details
   - `create <name>` — interactive prompt, writes TOML to .aether/presets/
   - `delete <name>` — removes user preset file (cannot delete built-in)

7) Integrate with R.1's `aether context`:
   - `--preset <name>` loads preset, applies settings
   - Explicit CLI flags override preset values
   - Task template: replace {target} with the file/symbol arguments

8) Add tests:
   - Preset loading from TOML
   - CLI flag overrides preset values
   - User preset overrides built-in
   - Task template variable substitution
   - Invalid TOML error handling

9) Run:
   - cargo fmt --all --check
   - cargo clippy -p aetherd -- -D warnings
   - cargo test -p aetherd

10) Commit: "Add prompt preset library for reusable context configurations"
```

---

## Stage R.4 — Multi-Format Output

**Codename:** Translator
**Depends on:** R.1 (Context Export)
**New crates:** None
**Modified crates:** `aetherd` (output formatters)

### Purpose

Different AI tools consume context differently. Markdown works for ChatGPT/Claude web. XML with structured tags works better for Claude API prompts. JSON works for programmatic consumption. Add format-specific output renderers.

### What to Build

#### Formats

| Format | Flag | Use Case | Structure |
|--------|------|----------|-----------|
| `markdown` | `--format markdown` (default) | ChatGPT, Claude web, Gemini | Headers, tables, code fences |
| `xml` | `--format xml` | Claude API prompts, structured context | `<aether_context>`, `<symbol>`, `<sir>` tags |
| `json` | `--format json` | Programmatic consumption, piping to tools | Structured JSON matching ExportContext |
| `compact` | `--format compact` | Maximum density, small budgets | SIR-only, no source code, no tables |

#### XML Format Example

```xml
<aether_context workspace="/home/rephu/projects/aether" generated="2026-03-15T14:30:00Z">
  <overview symbols="3748" sir_coverage="100%" health="42/100" />

  <target file="src/payments/processor.rs">
    <symbol name="validate_payment_amount" kind="function" health="78">
      <sir>
        <intent>Validates payment amounts against account balance</intent>
        <edge_cases>Zero amount, negative amount, exceeds balance, overflow</edge_cases>
        <error_handling>Returns Result&lt;ValidatedAmount, PaymentError&gt;</error_handling>
        <dependencies>AccountBalance, PaymentError, ValidatedAmount</dependencies>
      </sir>
      <source start_line="45" end_line="78">
        <!-- source code here -->
      </source>
    </symbol>
  </target>

  <graph depth="2">
    <edge source="handle_payment_request" target="validate_payment_amount" type="CALLS" />
    <edge source="process_payment" target="authorize_payment" type="CALLS" />
  </graph>

  <coupling>
    <pair file_a="processor.rs" file_b="validator.rs" score="0.87" />
  </coupling>

  <warnings>
    <health symbol="process_payment" issue="boundary_leaker" detail="calls into 2 communities" />
    <test_gap symbol="validate_payment_amount" sir_edge_cases="7" test_guards="3" />
  </warnings>
</aether_context>
```

#### Compact Format Example

```
=== AETHER Context: src/payments/processor.rs ===
Budget: 8K | Symbols: 7 | Health: 78/100

[validate_payment_amount] Function
  Intent: Validates payment amounts against account balance
  Edges: Zero/negative → InvalidAmount, exceeds balance → InsufficientFunds
  Deps: AccountBalance, PaymentError, ValidatedAmount
  Callers: handle_payment_request (src/api/routes.rs)
  Callees: AccountBalance::available (src/models/account.rs)

[process_payment] Function
  Intent: Orchestrates payment flow: validate → authorize → capture → record
  ...
```

### Pass Criteria

1. Each format produces valid, well-structured output.
2. XML is valid XML (parseable by standard XML parsers).
3. JSON matches a documented schema.
4. Compact format fits 2x more symbols in the same token budget as markdown.
5. All formats include budget usage metadata.
6. `cargo fmt`, `cargo clippy`, `cargo test` pass.

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
Focus on Stage R.4 — Multi-Format Output.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) git worktree add -b feature/phase-repo-stage-r4-formats /home/rephu/phase-repo-r4
3) cd /home/rephu/phase-repo-r4

4) Define OutputFormatter trait:
   - fn format(context: &ExportContext) -> String
   - Implementations: MarkdownFormatter, XmlFormatter, JsonFormatter, CompactFormatter

5) Implement each formatter:
   - Markdown: existing from R.1 (extract into trait impl)
   - XML: structured <aether_context> document with nested elements
   - JSON: serde_json serialization of ExportContext
   - Compact: dense single-line-per-symbol format

6) Wire --format flag to formatter selection in aether context command

7) Add tests:
   - Each format produces non-empty output
   - XML parses as valid XML
   - JSON parses as valid JSON with expected schema
   - Compact uses fewer tokens than markdown for same content

8) Run:
   - cargo fmt --all --check
   - cargo clippy -p aetherd -- -D warnings
   - cargo test -p aetherd

9) Commit: "Add multi-format output for context export (XML, JSON, compact)"
```

---

## Stage R.5 — Interactive Context Builder

**Codename:** Workbench
**Depends on:** R.1 (Context Export), R.2 (File Slicing) recommended
**New crates:** None
**Modified crates:** `aether-dashboard` (new page), `aetherd` (API endpoint)

### Purpose

Add a dashboard page (and optionally a TUI) where developers can interactively build context: browse the file tree, select symbols, see live token budget consumption, toggle intelligence layers, and copy the result. This is AETHER's answer to RepoPrompt's visual context builder — but powered by real semantic intelligence.

### What to Build

#### Dashboard Page: `/dashboard/context-builder`

**Layout:**

```
┌─────────────────────────────────────────────────────────────────┐
│ Context Builder                    Budget: [====----] 18K / 32K │
├──────────────────┬──────────────────────────────────────────────┤
│ File Tree        │ Selected Context Preview                     │
│ ☐ src/           │                                              │
│   ☑ payments/    │ ## src/payments/processor.rs                  │
│     ☑ proc...rs  │ ### validate_payment_amount (Function)        │
│     ☐ valid..rs  │ **Intent:** Validates payment amounts...      │
│   ☐ api/         │ ...                                          │
│   ☐ models/      │                                              │
│                  │ ## Dependency Neighborhood                    │
│ ──────────────── │ ...                                          │
│ Intelligence     │                                              │
│ ☑ SIR            │                                              │
│ ☑ Source Code    │                                              │
│ ☑ Graph          │                                              │
│ ☐ Coupling       │                                              │
│ ☑ Health         │                                              │
│ ☐ Drift          │                                              │
│ ☐ Memory         │                                              │
│ ──────────────── │                                              │
│ Task: [________] │                                              │
│ Depth: [2   ▼]  │                                              │
│ Format: [md  ▼] │                                              │
│ [Copy] [Export]  │                                              │
├──────────────────┴──────────────────────────────────────────────┤
│ Budget: SIR 4.2K | Source 9.8K | Graph 2.1K | Health 1.9K      │
└─────────────────────────────────────────────────────────────────┘
```

**Interactions:**
- Check/uncheck files in tree → live token count updates
- Toggle intelligence layers → preview updates
- Click symbol in preview → expand/collapse SIR details
- "Copy" button → copies formatted output to clipboard (via Clipboard API)
- "Export" button → downloads as file
- Task field → biases context selection via semantic search
- Preset dropdown → loads saved presets from R.3

#### API Endpoint

```
POST /api/v1/context/build
{
  "targets": ["src/payments/processor.rs"],
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
    "used": 28412,
    "by_layer": { "sir": 4200, "source": 9800, ... }
  },
  "token_estimate": 28412
}
```

### Pass Criteria

1. Dashboard page renders file tree with checkboxes.
2. Selecting files updates token budget display in real-time (via HTMX partial swap).
3. Intelligence layer toggles update preview.
4. Copy button copies formatted context to clipboard.
5. Preset dropdown loads R.3 presets.
6. Budget breakdown shows per-layer token usage.
7. Mobile-friendly layout (stacked instead of side-by-side).
8. `cargo fmt`, `cargo clippy`, `cargo test` pass.

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
Focus on Stage R.5 — Interactive Context Builder.

1) Ensure working tree is clean. If not, stop and report dirty files.
2) git worktree add -b feature/phase-repo-stage-r5-context-builder /home/rephu/phase-repo-r5
3) cd /home/rephu/phase-repo-r5

4) Add POST /api/v1/context/build endpoint to aether-dashboard:
   - Accept ExportContextRequest JSON body
   - Call context assembly engine from R.1
   - Return formatted content + budget usage breakdown

5) Add dashboard page /dashboard/context-builder:
   - File tree with checkboxes (load via HTMX from existing file list API)
   - Intelligence layer toggles (checkboxes)
   - Task input field, depth selector, format selector
   - Preview panel (HTMX partial swap on selection change)
   - Live token budget bar
   - Budget breakdown footer (per-layer token usage)
   - Copy button (navigator.clipboard.writeText)
   - Export/download button
   - Preset dropdown (loads from /api/v1/presets endpoint)

6) Add sidebar link for Context Builder page

7) Add tests:
   - API endpoint returns expected JSON schema
   - Budget usage breakdown sums correctly
   - Format flag produces correct output format

8) Run:
   - cargo fmt --all --check
   - cargo clippy -p aetherd --features dashboard -- -D warnings
   - cargo test -p aetherd

9) Commit: "Add interactive context builder dashboard page"
```

---

## Estimated Effort

| Stage | Codex Runs | Calendar Time | Priority |
|-------|------------|---------------|----------|
| R.1 Context Export CLI | 1–2 | 3–5 days | **Highest — do first** |
| R.2 SIR-Guided File Slicing | 1 | 1–2 days | High |
| R.3 Prompt Preset Library | 1–2 | 2–3 days | Medium |
| R.4 Multi-Format Output | 1 | 1–2 days | Medium |
| R.5 Interactive Context Builder | 1–2 | 3–5 days | Lower (nice-to-have) |
| **Total** | **5–8** | **~2–3 weeks** | |

---

## Sequencing Relative to Other Phases

```
Phase 8 remaining (health inversion + boundary leaker + 8.14b)
    ↓
Phase 10 (Conductor — batch, continuous, agent hooks)
    ↓
Phase Repo R.1 (Context Export) ← can start here once 10.3 ships
    ↓                               or even during 10.x if time permits
Phase Repo R.2–R.4 (parallel)
    ↓
Phase 9 (Beacon — Tauri app, integrates context builder into desktop)
    ↓
Phase Repo R.5 (Interactive Context Builder — may fold into Phase 9)
```

**Note on Phase 10.3 overlap:** Phase 10.3 (Agent Hooks) defines `sir context` which does token-budgeted context assembly for Claude Code integration. Phase Repo R.1 builds on the same engine but formats for human consumption (clipboard/paste) rather than programmatic consumption (MCP tool response). If Phase 10.3 ships first, R.1 reuses its context assembly. If Phase Repo ships first, 10.3 reuses R.1's assembly engine with a different output format. Either ordering works — the context assembly engine is the shared foundation.

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Context assembly queries too slow for interactive use | Medium | Medium | R.5's dashboard uses debounced requests. CLI (R.1) can be synchronous without concern. Cache assembled context in memory for repeated queries with same parameters. |
| Token estimation inaccuracy | Low | Low | The 4-chars-per-token heuristic is conservative. Users can adjust budget if output is too large/small. Exact counting is not worth the tiktoken dependency. |
| Preset proliferation / management overhead | Low | Low | Ship only 4 built-in presets. User presets are opt-in. TOML files are human-readable and deletable. |
| Format maintenance burden (4 formats) | Low | Medium | All formats render from the same ExportContext struct. Adding a field to ExportContext automatically appears in JSON. Other formats need manual update but the rendering code is simple. |

---

## What Phase Repo Does NOT Do

- **Does not replace MCP tools.** MCP remains the primary interface for connected agents. Phase Repo serves the disconnect case (paste into chat).
- **Does not add new intelligence.** Every layer in the context output already exists — SIR, graph, coupling, health, drift, memory, tests. Phase Repo is a read-only formatter.
- **Does not do agent orchestration.** RepoPrompt's Context Builder uses an agent to discover files. AETHER's context assembly is deterministic (graph traversal + priority ranking). If you specify a target, AETHER knows exactly what's relevant without an agent.
- **Does not write files.** This is pure read. No file mutation, no apply mode. That's Phase 8 Synthesizer territory.
