use super::{
    TemplateContext, languages_inline, plain_tool_list, recommended_actions, required_actions,
    search_modes_line, verify_commands_plain,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct CodexInstructionsTemplate;

impl CodexInstructionsTemplate {
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
            "AETHER CODE INTELLIGENCE INSTRUCTIONS\n\nAgent schema version: {}\nInference provider: {}\nMCP binary hint: {}\n\nAvailable tools:\n{}\n\nAvailable languages: {}\nSearch modes: {}\n\nRequired actions (mandatory):\n{}\n\nRecommended actions (advisory):\n{}\n\nStaleness guidance:\nIf you have made many rapid edits, call aether_status before trusting retrieval results. If counts look stale, wait for indexing or trigger aether_index_once.\n\nVerify commands:\n{}\n",
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
