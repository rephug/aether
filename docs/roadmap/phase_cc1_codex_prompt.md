# Claude Code Prompt — Stage CC.1: init-agent Slash Commands + CLAUDE.md Audit Section

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
git worktree add -B feature/cc1-slash-commands /home/rephu/feature/cc1-slash-commands
cd /home/rephu/feature/cc1-slash-commands
```

---

## Context

AETHER's `init-agent` command generates agent configuration files for Claude Code,
Codex, and Cursor. Currently it generates:

- `CLAUDE.md` — project context and MCP tool reference
- `.agents/skills/aether-context/SKILL.md` — workflow skill
- `.codex-instructions` — Codex agent instructions
- `.cursor/rules` — Cursor rules

We need to extend it to also generate Claude Code slash commands that provide
zero-friction entry points for AETHER-powered auditing and refactoring.

Claude Code slash commands are markdown files in `.claude/commands/`. The filename
becomes the command name. They support YAML frontmatter with `argument-hint` and
`description` fields, plus markdown body containing instructions.

---

## Source Inspection (MANDATORY — do this before writing any code)

1. Read `crates/aetherd/src/init_agent.rs` — understand `files_for_platform()`,
   `GeneratedFile`, `InitAgentOutcome`, and the test structure.

2. Read `crates/aetherd/src/templates/mod.rs` — understand `TemplateContext`,
   `TOOL_DESCRIPTIONS`, the helper functions (`languages_inline`, `verify_commands_markdown`,
   `search_modes_line`, `required_actions`, `recommended_actions`, `markdown_tool_list`),
   and the existing template traits.

3. Read `crates/aetherd/src/templates/claude_md.rs` — understand how `ClaudeTemplate::render`
   builds the CLAUDE.md content. Note the format string approach.

4. Read `crates/aetherd/src/templates/skill_md.rs` — understand the pattern for
   template structs with `render(&TemplateContext) -> String`.

5. Run `grep -c "tool.*name.*=" crates/aether-mcp/src/tools/router.rs` to count
   actual MCP tools registered. Compare against `TOOL_DESCRIPTIONS` in templates/mod.rs —
   the array is outdated and missing many tools.

6. Check which tools exist as MCP tools vs CLI-only:
   ```bash
   grep '#\[tool(' crates/aether-mcp/src/tools/router.rs | grep 'name'
   ```
   Note: `sir_inject` and `sir_context` are CLI-only, NOT MCP tools.

---

## Implementation

### Step 1: Add slash command template modules

Create three new files in `crates/aetherd/src/templates/`:

**`crates/aetherd/src/templates/audit_cmd.rs`**

```rust
use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditCommandTemplate;

impl AuditCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        // The content below is a Claude Code slash command.
        // $ARGUMENTS is replaced by Claude Code with user input after /audit.
        r#"---
argument-hint: <crate-or-file>
description: Deep audit for bugs using AETHER structural intelligence
---

Audit $ARGUMENTS for bugs using AETHER's MCP tools to guide analysis.

## Step 1: Find audit targets
1. Call `aether_health` scoped to $ARGUMENTS to get structural risk scores.
2. Symbols with high risk_score, high betweenness centrality, or low test coverage
   are priority targets.
3. If `aether_audit_candidates` is available, call it with the scope for a
   pre-ranked target list with reasoning hints.
4. If neither tool returns results, fall back to `aether_symbol_lookup` for
   the target file/crate and prioritize large, complex symbols.

## Step 2: Deep analysis for each target symbol
For each high-risk symbol:
1. Call `aether_get_sir` to read the current SIR annotation (intent, error_modes,
   confidence, reasoning_trace).
2. Read the actual source file to inspect full implementation.
3. If reasoning_trace mentions uncertainty, investigate that specific concern.
4. Check for these bug categories:
   - ARITHMETIC: overflow, underflow, NaN propagation, saturating ops that
     invert ordering, negative values in unsigned contexts, exact float comparison
   - ENCODING: UTF-8 boundary slicing, byte vs char indexing, lossy conversions
   - SILENT_FAILURE: unwrap_or_default hiding errors, .ok() discarding info,
     catch-all match arms, Ok(empty) masking errors from callers
   - STATE: stale values carried across iterations, mutable refs outliving scope
   - TYPE_SAFETY: accepting any value when only specific ones are valid,
     no validation on deserialized data
   - CONCURRENCY: mutex poisoning, lock ordering, send across thread boundaries
   - RESOURCE_LEAK: unclosed file handles, connections not returned to pool
   - LOGIC_ERROR: off-by-one, wrong assertion values, inverted conditions

## Step 3: Cross-symbol analysis
When you find a suspicious symbol, check its callers and callees:
1. Call `aether_dependencies` on the symbol to get callers and callees.
2. Call `aether_get_sir` on callers — do they handle the error mode you found?
3. Call `aether_get_sir` on callees — do they have complementary issues?
4. If a data flow crosses multiple symbols, trace it end-to-end and document
   any boundary where assumptions change.

## Step 4: Record results
For each finding:
1. Call `aether_sir_inject` with an improved SIR annotation. Set generation_pass
   to "deep". Include discovered error_modes. If `aether_sir_inject` is not
   available as an MCP tool, run via bash:
   `aetherd sir-inject <symbol-name> --intent "..." --edge-cases "..." --force`
2. If `aether_audit_submit` is available, submit a structured finding with
   severity, category, certainty, trigger condition, and impact.
3. Otherwise, call `aether_remember` with a structured note:
   ```
   AUDIT FINDING: [severity] [category]
   Symbol: [qualified_name]
   File: [file_path]:[line_number]
   Description: [what's wrong]
   Trigger: [what input/condition causes it]
   Impact: [what happens]
   Certainty: confirmed | suspected | theoretical
   ```

Summarize all findings at the end with severity counts.
"#.to_owned()
    }
}
```

**`crates/aetherd/src/templates/refactor_cmd.rs`**

```rust
use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct RefactorCommandTemplate;

impl RefactorCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
argument-hint: <crate-or-file>
description: Find refactoring opportunities using AETHER health and community data
---

Analyze $ARGUMENTS for refactoring opportunities using AETHER.

## Step 1: Health assessment
1. Call `aether_health` for the target to get structural risk scores.
2. Look for: high complexity, low cohesion, symbols with high betweenness
   (bottlenecks), dependency cycles, and orphaned code.
3. Call `aether_health_explain` if a crate-level summary is useful.

## Step 2: God file detection
1. Look for files exceeding 500 lines of real code.
2. Call `aether_health_hotspots` to find the hottest crates by health score.

## Step 3: Trait decomposition
If the target contains large traits or impl blocks:
1. Call `aether_suggest_trait_split` to get clustering-based split suggestions.
2. Call `aether_usage_matrix` for a detailed consumer-by-method breakdown.

## Step 4: Community and boundary analysis
1. Check which community each symbol belongs to.
2. Look for symbols that bridge multiple communities — these are boundary
   leakers that might belong in a different module.
3. Call `aether_drift_report` to detect structural anomalies.

## Step 5: Propose refactoring plan
For each refactoring opportunity, explain:
- What to extract, split, or move
- Why (backed by AETHER metrics: health score, betweenness, consumer count)
- Risk level (how many callers affected — use `aether_blast_radius`)
- Suggested order of operations

## Step 6: Prepare for refactoring
If proceeding with refactoring:
1. Call `aether_refactor_prep` to snapshot current SIR intents.
2. After refactoring, call `aether_verify_intent` to confirm no semantic drift.

Record significant findings via `aether_remember`.
"#.to_owned()
    }
}
```

**`crates/aetherd/src/templates/audit_report_cmd.rs`**

```rust
use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditReportCommandTemplate;

impl AuditReportCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
description: Show all audit findings from previous sessions
---

Retrieve and display all audit findings.

## If `aether_audit_report` is available:
Call `aether_audit_report` to retrieve all open findings.
If the user specifies a crate, severity, or status filter, pass those parameters.

## Otherwise:
Call `aether_recall` with query "AUDIT FINDING" to find stored findings.
Parse the structured notes and group by crate, then by severity.

## Display format
Show findings grouped by crate, then severity (critical → high → medium → low).
For each finding show: symbol, file, severity, category, description, certainty, status.
End with a summary table of counts by severity.
"#.to_owned()
    }
}
```

### Step 2: Register templates in mod.rs

In `crates/aetherd/src/templates/mod.rs`:

1. Add module declarations:
   ```rust
   pub mod audit_cmd;
   pub mod audit_report_cmd;
   pub mod refactor_cmd;
   ```

2. Add re-exports:
   ```rust
   pub use audit_cmd::AuditCommandTemplate;
   pub use audit_report_cmd::AuditReportCommandTemplate;
   pub use refactor_cmd::RefactorCommandTemplate;
   ```

3. Update `TOOL_DESCRIPTIONS` to include the audit-relevant tools that are
   currently missing. Inspect the router to get the actual list of MCP tools
   and add any that are missing from the array. At minimum, add entries for:
   - `aether_health_hotspots`
   - `aether_health_explain`
   - `aether_refactor_prep`
   - `aether_verify_intent`
   - `aether_drift_report`
   - `aether_blast_radius`
   - `aether_trace_cause`
   - `aether_remember`
   - `aether_recall`
   - `aether_ask`
   - `aether_session_note`
   - `aether_test_intents`
   - `aether_suggest_trait_split`
   - `aether_usage_matrix`
   - `aether_acknowledge_drift`

   Use the `description` text from the `#[tool(description = "...")]` attributes
   in router.rs as the source of truth for each description.

### Step 3: Add audit section to CLAUDE.md template

In `crates/aetherd/src/templates/claude_md.rs`, add an "Audit Workflow" section
to the rendered output. Insert it after the "Recommended Actions" section and
before "Staleness Guidance". The section should contain:

```markdown
## Audit Workflow

When asked to audit code for bugs, use AETHER MCP tools to guide deep analysis.

### Finding targets
- Call `aether_health` for the target file or crate to get structural risk scores.
- Symbols with high risk_score, high betweenness, or low test_count are priority targets.
- Use `/audit <target>` for a guided audit workflow.

### Recording results
- Call `aether_remember` with structured AUDIT FINDING notes.
- If `aether_audit_submit` is available, prefer it for structured queryable findings.

### Refactoring workflow
- Call `aether_refactor_prep` before refactoring to snapshot intents.
- Call `aether_verify_intent` after refactoring to detect semantic drift.
- Use `/refactor <target>` for a guided refactoring workflow.
```

### Step 4: Extend init_agent.rs to emit command files

In `crates/aetherd/src/init_agent.rs`, update `files_for_platform()`:

In the `AgentPlatform::Claude | AgentPlatform::All` match arm, add three
`GeneratedFile` entries after the existing CLAUDE.md and skill entries:

```rust
files.push(GeneratedFile {
    relative_path: PathBuf::from(".claude/commands/audit.md"),
    content: AuditCommandTemplate::render(context),
});
files.push(GeneratedFile {
    relative_path: PathBuf::from(".claude/commands/refactor.md"),
    content: RefactorCommandTemplate::render(context),
});
files.push(GeneratedFile {
    relative_path: PathBuf::from(".claude/commands/audit-report.md"),
    content: AuditReportCommandTemplate::render(context),
});
```

Add the necessary `use` import for the new templates.

### Step 5: Update tests

In the `tests` module of `init_agent.rs`:

1. Update `init_agent_creates_expected_files_for_all_platforms` to assert that
   `.claude/commands/audit.md`, `.claude/commands/refactor.md`, and
   `.claude/commands/audit-report.md` all exist.

2. Update `init_agent_skips_existing_files_without_force` — the test seeds
   `CLAUDE.md` and checks it's skipped. Confirm command files ARE written
   (they didn't exist before).

3. Add a new test `generated_commands_contain_expected_content`:
   ```rust
   #[test]
   fn generated_commands_contain_expected_content() {
       let temp = tempdir().expect("tempdir");
       let workspace = temp.path();
       write_config_with_embeddings(workspace, true);
       run_init_agent(workspace, InitAgentOptions {
           platform: AgentPlatform::Claude,
           force: false,
       }).expect("init-agent should succeed");

       let audit = fs::read_to_string(workspace.join(".claude/commands/audit.md"))
           .expect("read audit command");
       assert!(audit.contains("argument-hint:"));
       assert!(audit.contains("aether_health"));
       assert!(audit.contains("ARITHMETIC"));
       assert!(audit.contains("aether_audit_submit"));

       let refactor = fs::read_to_string(workspace.join(".claude/commands/refactor.md"))
           .expect("read refactor command");
       assert!(refactor.contains("aether_suggest_trait_split"));
       assert!(refactor.contains("aether_refactor_prep"));

       let report = fs::read_to_string(workspace.join(".claude/commands/audit-report.md"))
           .expect("read audit-report command");
       assert!(report.contains("aether_audit_report"));
       assert!(report.contains("aether_recall"));
   }
   ```

4. Update `generated_claude_contains_schema_version` or add a new test to verify
   the CLAUDE.md contains "Audit Workflow".

---

## Scope guard

**Only modify files in `crates/aetherd/src/templates/` and `crates/aetherd/src/init_agent.rs`.**

Do NOT modify any MCP tools, store code, schema, or other crates.

---

## Validation gate

```bash
cargo fmt --all --check
cargo clippy -p aetherd -- -D warnings
cargo test -p aetherd
```

Do NOT run `cargo test --workspace` — OOM risk.

All three must pass before committing.

---

## Commit

```bash
git add -A
git commit -m "feat(init-agent): generate /audit, /refactor, /audit-report slash commands + CLAUDE.md audit section"
```

**PR title:** `feat(init-agent): generate /audit, /refactor, /audit-report slash commands + CLAUDE.md audit section`

**PR body:**
```
Stage CC.1 of the Claude Code Audit Integration phase.

Changes:
- Generate three Claude Code slash commands via init-agent:
  /audit <target> — AETHER-guided deep bug audit
  /refactor <target> — health + community based refactoring analysis
  /audit-report — show findings from previous audit sessions
- Add Audit Workflow section to generated CLAUDE.md
- Update TOOL_DESCRIPTIONS to include all current MCP tools
- Commands degrade gracefully — fallback instructions for tools
  not yet available (aether_audit_candidates, aether_audit_submit)

Decisions: #103, #106
```

---

## Post-commit

```bash
git push origin feature/cc1-slash-commands
# Create PR via GitHub web UI with title + body above
# After merge:
git switch main && git pull --ff-only
git worktree remove /home/rephu/feature/cc1-slash-commands
git branch -D feature/cc1-slash-commands
```
