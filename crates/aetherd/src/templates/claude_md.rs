use super::{
    TOOLS_NOTE, TemplateContext, languages_inline, recommended_actions, required_actions,
    search_modes_line, verify_commands_markdown,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeTemplate;

impl ClaudeTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let required = required_actions(context)
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let recommended = recommended_actions(context)
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "# AETHER Code Intelligence\n\nThis project uses AETHER for local code intelligence. Use current source and tests to ground decisions; the MCP tools below are optional evidence aids.\n\n## Agent Schema Version: {}\n\n## Inference Provider\n- `{}`\n\n## MCP Binary Hint\n- `{}`\n\n## Tool Reference (check server availability)\n{}\n\n## Available Languages\n- {}\n\n## Search Modes\n- {}\n\n## Required Actions (mandatory)\n{}\n\n## Recommended Actions (advisory)\n{}\n\n## Audit Workflow\n\nFor an explicitly requested AETHER audit or annotation workflow, use available writable tools to guide and record deep analysis.\n\n### Finding targets\n- Call `aether_health` for the target file or crate to get structural risk scores.\n- Symbols with high risk_score, high betweenness, or low test_count are priority targets.\n- Use `/audit <target>` for a guided audit workflow.\n\n### Recording results\n- Call `aether_remember` with structured AUDIT FINDING notes.\n- If `aether_audit_submit` is available, prefer it for structured queryable findings.\n\n### Refactoring workflow\n- Call `aether_refactor_prep` before refactoring to snapshot intents.\n- Call `aether_verify_intent` after refactoring to detect semantic drift.\n- Use `/refactor <target>` for a guided refactoring workflow.\n\n## Zero-Key Onboarding\n\nNo API key is needed for baseline coverage. Index with the key-free mock provider, then let Claude Code sessions replace the `[MOCK]` placeholders (confidence 0.1) with scan-level SIRs, then deepen:\n\n```\naetherd --workspace . --index-once --full --inference-provider mock\n./scripts/scan_all.sh\naetherd --workspace . regenerate --deep --below-confidence 0.85\n```\n\n`/scan <crate> [batch-size]` does one crate at a time (10 symbols per turn, grouped by file); `init-agent` writes `.claude/commands/scan.md`, `scripts/scan_all.sh` and the project-scoped `.mcp.json` the scan sessions need. Any provider configured under `[inference]` can produce the baseline instead of the mock provider; that choice belongs to the project.\n\n## Staleness Guidance\nIf available, `aether_status` supplies diagnostics; counts alone cannot establish freshness or complete dependencies. Compare current source/Git with indexed evidence. When an index refresh is explicitly requested, run `aetherd --workspace . --index-once` and inspect any errors or omissions.\n\n## Verify Commands\n{}\n",
            context.agent_schema_version,
            context.inference_provider,
            context.mcp_binary_hint,
            TOOLS_NOTE,
            languages_inline(context),
            search_modes_line(context),
            required,
            recommended,
            verify_commands_markdown(context)
        ) + SLASH_COMMANDS_SECTION
    }
}

/// Appended after the rest of the guidance: the generated slash commands and the
/// workflow that strings them together.
const SLASH_COMMANDS_SECTION: &str = "
## Slash Commands

`init-agent` writes these Claude Code commands to `.claude/commands/`:

- `/explain-area <target>`: a plain-prose explanation of a file, directory, crate or symbol.
- `/pre-flight <task>`: a briefing before implementing a task: what exists, the constraints, what could break and a recommended approach.
- `/impact <symbol>`: the blast radius of changing a symbol, with a risk rating.
- `/review [base-branch]`: a semantic review of the current branch against SIRs, callers and contracts.
- `/context <target>`: a self-contained, plain-language briefing on an area, to paste into any AI chat.
- `/health-check`: a 30-second overview of coverage, SIR confidence and the least healthy crates.
- `/audit <crate-or-file>`, `/audit-changes [commit-range]`, `/audit-report`: bug audits and their recorded findings.
- `/refactor <crate-or-file>`, `/refactor-deep [file-path]`: refactoring with intent snapshots.
- `/scan <crate> [batch-size]`, `/enrich <crate-name>`, `/next`: SIR coverage, deeper SIRs, and what to work on next.

Recommended workflow: `/explain-area` for an unfamiliar area, then `/pre-flight <task>`, implement, then `/review`. Run `/impact <symbol>` before changing a widely used symbol, and `/health-check` at the start of a session.
";
