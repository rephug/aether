use super::{
    TOOLS_NOTE, TemplateContext, languages_inline, recommended_actions, required_actions,
    search_modes_line, verify_commands_plain,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct CodexInstructionsTemplate;

impl CodexInstructionsTemplate {
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
            "AETHER CODE INTELLIGENCE INSTRUCTIONS\n\nAgent schema version: {}\nInference provider: {}\nMCP binary hint: {}\n\nTool reference (check server availability):\n{}\n\nAvailable languages: {}\nSearch modes: {}\n\nRequired actions (mandatory):\n{}\n\nRecommended actions (advisory):\n{}\n\nStaleness guidance:\nIf available, aether_status supplies diagnostics; counts alone cannot establish freshness or complete dependencies. Compare current source/Git with indexed evidence. When an index refresh is explicitly requested, run aetherd --workspace . --index-once and inspect any errors or omissions.\n\nVerify commands:\n{}\n",
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
