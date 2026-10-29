use super::{
    TemplateContext, recommended_actions, required_actions, search_modes_line,
    verification_guidance, verify_commands_markdown,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SkillTemplate;

impl SkillTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let required = required_actions(context)
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let optional = recommended_actions(context)
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "---\nname: aether-context\ndescription: \"Optional code intelligence evidence for AETHER-indexed projects.\"\n---\n\n# AETHER Code Intelligence Workflow\n\n## When to use\n- Exploring unfamiliar code or recorded history with available AETHER tools\n- An explicitly requested AETHER audit or annotation workflow\n\n## Workflow: Orient -> Discover -> Understand -> Modify -> Verify\n\n### 1. Orient\nInspect current source, Git state and project instructions. Optional status diagnostics cannot prove index freshness or completeness.\n\n### 2. Discover\nUse ordinary repository search and any useful advertised AETHER tools. Available search modes by configuration: {}\n\n### 3. Understand\nCompare annotation/history evidence with current source, callers and relevant tests; distinguish recorded reasons from generated descriptions.\n\n### 4. Modify\nPreserve intended behavior established by source, tests and recorded contracts. Annotation writes belong to explicitly requested workflows with writable tools.\n\n### 5. Verify\n{}\nPreferred configured commands:\n{}\n\n## Required repository checks\n{}\n\n## Optional AETHER guidance\n{}\n\n## Anti-patterns to avoid\n- Do not infer full dependency coverage from missing edges or status counts.\n- Do not present a SIR difference as a recorded design reason.\n- Do not assume semantic search is available from configuration alone.\n- Report blocked validation and its limits.\n",
            search_modes_line(context),
            verification_guidance(context),
            verify_commands_markdown(context),
            required,
            optional,
        )
    }
}
