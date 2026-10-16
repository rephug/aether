use super::{
    GET_SIR_GUIDANCE, STATUS_GUIDANCE, TemplateContext, required_actions, search_modes_line,
    verify_commands_markdown,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SkillTemplate;

impl SkillTemplate {
    pub fn render(context: &TemplateContext) -> String {
        let verify_step = if context.verification_tool {
            "Run `aether_verify` before completing the task."
        } else {
            "Run the project's verify commands before completing the task."
        };
        let required = required_actions(context)
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "---\nname: aether-context\ndescription: \"Code intelligence workflow for AETHER-indexed projects. Use before modifying, refactoring, or reviewing unfamiliar code.\"\n---\n\n# AETHER Code Intelligence Workflow\n\n## When to activate\n- Any task involving code modification, refactoring, or review\n- Onboarding to unfamiliar modules or symbols\n- Investigating why code changed recently\n- Tracing dependencies before making risky edits\n\n## Workflow: Orient -> Discover -> Understand -> Modify -> Verify\n\n### 1. Orient\n- {}\n\n### 2. Discover\n- Use `aether_search` in the best available mode for this project.\n- Available search modes: {}\n\n### 3. Understand\n- {}\n- Call `aether_symbol_timeline` for recent change history.\n- Call `aether_call_chain` when blast radius is unclear.\n\n### 4. Modify\n- Preserve intended side effects and dependency contracts from SIR.\n- Avoid silent behavior changes in error handling paths.\n\n### 5. Verify\n- {}\n- Preferred commands:\n{}\n\n## Required safety rules\n{}\n\n## Anti-patterns to avoid\n- Do not guess side effects when `aether_get_sir` can confirm them.\n- Do not revert recent edits without checking why they were made: `git log` holds the reasons, and `aether_why_changed` shows how the SIR changed.\n- Do not assume semantic search is available when embeddings are disabled.\n- Do not skip verification on modified code paths.\n",
            STATUS_GUIDANCE,
            search_modes_line(context),
            GET_SIR_GUIDANCE,
            verify_step,
            verify_commands_markdown(context),
            required
        )
    }
}
