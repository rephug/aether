pub mod claude_md;
pub mod codex_instructions;
pub mod cursor_rules;
pub mod skill_md;

pub use claude_md::ClaudeTemplate;
pub use codex_instructions::CodexInstructionsTemplate;
pub use cursor_rules::CursorRulesTemplate;
pub use skill_md::SkillTemplate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateContext {
    pub languages: Vec<String>,
    pub verify_commands: Vec<String>,
    pub embeddings_enabled: bool,
    pub inference_provider: String,
    pub agent_schema_version: u32,
    pub mcp_binary_hint: String,
}

pub(crate) const TOOL_DESCRIPTIONS: [(&str, &str); 10] = [
    (
        "aether_status",
        "check local index/store freshness and symbol/SIR counts",
    ),
    (
        "aether_symbol_lookup",
        "find symbols by name/path/language using lexical lookup",
    ),
    (
        "aether_search",
        "search symbols using lexical, semantic, or hybrid ranking",
    ),
    (
        "aether_symbol_timeline",
        "inspect how a symbol changed across SIR versions",
    ),
    (
        "aether_why_changed",
        "compare symbol versions and summarize what changed",
    ),
    (
        "aether_get_sir",
        "fetch canonical SIR for symbol/file/module before edits",
    ),
    (
        "aether_dependencies",
        "list direct callers and dependencies for a symbol",
    ),
    (
        "aether_call_chain",
        "trace call-chain levels to understand blast radius",
    ),
    (
        "aether_verify",
        "run configured verification commands after code changes",
    ),
    (
        "aether_explain",
        "summarize intent for a symbol or file-level rollup",
    ),
];

pub(crate) fn languages_inline(context: &TemplateContext) -> String {
    if context.languages.is_empty() {
        "no indexed languages detected".to_owned()
    } else {
        context.languages.join(", ")
    }
}

pub(crate) fn verify_commands_markdown(context: &TemplateContext) -> String {
    if context.verify_commands.is_empty() {
        return "- no verify commands configured".to_owned();
    }

    context
        .verify_commands
        .iter()
        .map(|command| format!("- `{command}`"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn verify_commands_plain(context: &TemplateContext) -> String {
    if context.verify_commands.is_empty() {
        return "- no verify commands configured".to_owned();
    }

    context
        .verify_commands
        .iter()
        .map(|command| format!("- {command}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn search_modes_line(context: &TemplateContext) -> String {
    if context.embeddings_enabled {
        "lexical, semantic, hybrid (semantic search is enabled)".to_owned()
    } else {
        "lexical only (enable embeddings in .aether/config.toml for semantic and hybrid)".to_owned()
    }
}

pub(crate) fn required_actions() -> [&'static str; 4] {
    [
        "Always call `aether_get_sir` before reverting, deleting, or refactoring symbols.",
        "Always call `aether_why_changed` before reverting recent changes.",
        "Always call `aether_verify` after modifying code.",
        "If `aether_verify` fails, fix the issue before proceeding.",
    ]
}

pub(crate) fn recommended_actions(context: &TemplateContext) -> Vec<String> {
    let search_guidance = if context.embeddings_enabled {
        "Consider `aether_search` with hybrid mode when exploring unfamiliar code."
    } else {
        "Consider `aether_search` (lexical mode) when exploring unfamiliar code."
    };

    vec![
        search_guidance.to_owned(),
        "Consider `aether_symbol_timeline` when reviewing recent changes.".to_owned(),
        "Consider `aether_call_chain` when tracing dependencies and downstream impact.".to_owned(),
        "Call `aether_status` at task start to confirm index freshness.".to_owned(),
    ]
}

pub(crate) fn markdown_tool_list() -> String {
    TOOL_DESCRIPTIONS
        .iter()
        .map(|(name, description)| format!("- `{name}`: {description}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn plain_tool_list() -> String {
    TOOL_DESCRIPTIONS
        .iter()
        .map(|(name, description)| format!("- {name}: {description}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeTemplate, CodexInstructionsTemplate, CursorRulesTemplate, SkillTemplate,
        TemplateContext,
    };

    fn sample_context() -> TemplateContext {
        TemplateContext {
            languages: vec!["Rust".to_owned(), "Python".to_owned()],
            verify_commands: vec![
                "cargo test --workspace".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned(),
            ],
            embeddings_enabled: true,
            inference_provider: "mock".to_owned(),
            agent_schema_version: 7,
            mcp_binary_hint: "./target/debug/aether-mcp".to_owned(),
        }
    }

    #[test]
    fn claude_template_renders_context_values() {
        let rendered = ClaudeTemplate::render(&sample_context());

        assert!(rendered.contains("## Agent Schema Version: 7"));
        assert!(rendered.contains("Rust, Python"));
        assert!(rendered.contains("semantic search is enabled"));
        assert!(rendered.contains("cargo test --workspace"));
        assert!(rendered.contains("aether_get_sir"));
    }

    #[test]
    fn codex_template_renders_plain_text_instructions() {
        let rendered = CodexInstructionsTemplate::render(&sample_context());

        assert!(rendered.contains("AETHER CODE INTELLIGENCE INSTRUCTIONS"));
        assert!(rendered.contains("Agent schema version: 7"));
        assert!(rendered.contains("Inference provider: mock"));
        assert!(rendered.contains("semantic search is enabled"));
    }

    #[test]
    fn cursor_template_uses_numbered_directives() {
        let rendered = CursorRulesTemplate::render(&sample_context());

        assert!(rendered.contains("1. Before editing or refactoring"));
        assert!(rendered.contains("Agent schema version: 7"));
        assert!(rendered.contains("aether_verify"));
    }

    #[test]
    fn skill_template_contains_workflow_and_anti_patterns() {
        let rendered = SkillTemplate::render(&sample_context());

        assert!(rendered.contains("Orient -> Discover -> Understand -> Modify -> Verify"));
        assert!(rendered.contains("## Anti-patterns to avoid"));
        assert!(rendered.contains("aether_why_changed"));
    }

    #[test]
    fn templates_handle_missing_verify_commands_and_embeddings_disabled() {
        let mut context = sample_context();
        context.verify_commands.clear();
        context.embeddings_enabled = false;

        let rendered = ClaudeTemplate::render(&context);
        assert!(rendered.contains("no verify commands configured"));
        assert!(rendered.contains("lexical only"));
    }
}
