use super::{
    TemplateContext, languages_inline, markdown_tool_list, recommended_actions, required_actions,
    search_modes_line, verify_commands_markdown,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeTemplate;

impl ClaudeTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let required = required_actions()
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
            "# AETHER Code Intelligence\n\nThis project uses AETHER for local code intelligence. Use the MCP tools below to ground decisions before making risky edits.\n\n## Agent Schema Version: {}\n\n## Inference Provider\n- `{}`\n\n## MCP Binary Hint\n- `{}`\n\n## Available Tools\n{}\n\n## Available Languages\n- {}\n\n## Search Modes\n- {}\n\n## Required Actions (mandatory)\n{}\n\n## Recommended Actions (advisory)\n{}\n\n## Audit Workflow\n\nWhen asked to audit code for bugs, use AETHER MCP tools to guide deep analysis.\n\n### Finding targets\n- Call `aether_health` for the target file or crate to get structural risk scores.\n- Symbols with high risk_score, high betweenness, or low test_count are priority targets.\n- Use `/audit <target>` for a guided audit workflow.\n\n### Recording results\n- Call `aether_remember` with structured AUDIT FINDING notes.\n- If `aether_audit_submit` is available, prefer it for structured queryable findings.\n\n### Refactoring workflow\n- Call `aether_refactor_prep` before refactoring to snapshot intents.\n- Call `aether_verify_intent` after refactoring to detect semantic drift.\n- Use `/refactor <target>` for a guided refactoring workflow.\n\n## Staleness Guidance\nIf you have made many rapid edits, call `aether_status` before trusting retrieval results. If symbol or SIR counts look stale, wait for indexing or run `aether_index_once`.\n\n## Verify Commands\n{}\n",
            context.agent_schema_version,
            context.inference_provider,
            context.mcp_binary_hint,
            markdown_tool_list(),
            languages_inline(context),
            search_modes_line(context),
            required,
            recommended,
            verify_commands_markdown(context)
        )
    }
}
