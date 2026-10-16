use super::{
    TOOLS_NOTE, TemplateContext, languages_inline, recommended_actions, required_actions,
    search_modes_line, verify_commands_plain,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct CursorRulesTemplate;

impl CursorRulesTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let required = required_actions(context)
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
            "# AETHER Cursor Rules\n\nAgent schema version: {}\nInference provider: {}\nMCP binary hint: {}\n\n## Tool reference\n{}\n\n## Workspace context\n- Languages: {}\n- Search modes: {}\n\n## Mandatory directives\n{}\n\n## Recommended directives\n{}\n\n## Staleness guidance\nAfter heavy edit churn, call aether_status again before relying on retrieval output. freshness.modified_files, freshness.deleted_files and freshness.new_files count the source files changed since the last sync; if any is not zero and freshness.daemon_running is false, run aetherd --workspace . --index-once.\n\n## Verify commands\n{}\n",
            context.agent_schema_version,
            context.inference_provider,
            context.mcp_binary_hint,
            TOOLS_NOTE,
            languages_inline(context),
            search_modes_line(context),
            required,
            recommended,
            verify_commands_plain(context)
        )
    }
}
