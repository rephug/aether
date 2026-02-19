use super::{
    TemplateContext, languages_inline, plain_tool_list, recommended_actions, required_actions,
    search_modes_line, verify_commands_plain,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct CursorRulesTemplate;

impl CursorRulesTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let required = required_actions()
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{}. {}", index + 1, line.trim_end_matches('.')))
            .collect::<Vec<_>>()
            .join("\n");
        let recommended = recommended_actions(context)
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{}. {}", index + 1, line.trim_end_matches('.')))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "# AETHER Cursor Rules\n\nAgent schema version: {}\nInference provider: {}\nMCP binary hint: {}\n\n## Tool reference\n{}\n\n## Workspace context\n- Languages: {}\n- Search modes: {}\n\n## Mandatory directives\n1. Before editing or refactoring symbols, call aether_get_sir and inspect intent, side effects, and dependencies.\n2. Before reverting recent changes, call aether_why_changed.\n3. After code changes, call aether_verify and fix failures before concluding.\n4. Treat verification failures as blockers, not optional warnings.\n\n## Required actions (equivalent checklist)\n{}\n\n## Recommended directives\n{}\n\n## Staleness guidance\nIf the project has heavy edit churn, call aether_status before relying on retrieval output. If counts are stale, wait for indexing or trigger aether_index_once.\n\n## Verify commands\n{}\n",
            context.agent_schema_version,
            context.inference_provider,
            context.mcp_binary_hint,
            plain_tool_list(),
            languages_inline(context),
            search_modes_line(context),
            required,
            recommended,
            verify_commands_plain(context)
        )
    }
}
