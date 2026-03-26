pub mod audit_changes_cmd;
pub mod audit_cmd;
pub mod audit_report_cmd;
pub mod claude_md;
pub mod codex_instructions;
pub mod cursor_rules;
pub mod refactor_cmd;
pub mod skill_md;

pub use audit_changes_cmd::AuditChangesCommandTemplate;
pub use audit_cmd::AuditCommandTemplate;
pub use audit_report_cmd::AuditReportCommandTemplate;
pub use claude_md::ClaudeTemplate;
pub use codex_instructions::CodexInstructionsTemplate;
pub use cursor_rules::CursorRulesTemplate;
pub use refactor_cmd::RefactorCommandTemplate;
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

pub(crate) const TOOL_DESCRIPTIONS: [(&str, &str); 40] = [
    ("aether_status", "Get AETHER local store status"),
    (
        "aether_symbol_lookup",
        "Lookup symbols by qualified name or file path",
    ),
    (
        "aether_dependencies",
        "Get resolved callers and call dependencies for a symbol",
    ),
    (
        "aether_usage_matrix",
        "Get a consumer-by-method usage matrix for a trait or struct, showing which files call which methods and suggesting method clusters for trait decomposition",
    ),
    (
        "aether_suggest_trait_split",
        "Suggest how to decompose a large trait or struct into smaller capability groups based on consumer usage patterns",
    ),
    (
        "aether_call_chain",
        "Get transitive call-chain levels for a symbol",
    ),
    (
        "aether_search",
        "Search symbols by name, path, language, or kind",
    ),
    (
        "aether_remember",
        "Store project memory note content with deterministic deduplication",
    ),
    (
        "aether_session_note",
        "Capture an in-session project note with source_type=session",
    ),
    (
        "aether_recall",
        "Recall project memory notes using lexical, semantic, or hybrid retrieval",
    ),
    (
        "aether_ask",
        "Search symbols, notes, coupling, and test intents with unified ranking",
    ),
    (
        "aether_audit_candidates",
        "Get ranked list of symbols most in need of deep audit review, combining structural risk with SIR confidence and reasoning trace uncertainty",
    ),
    (
        "aether_audit_cross_symbol",
        "trace callers and callees with full SIR and source for cross-boundary audit",
    ),
    (
        "aether_audit_submit",
        "Submit a structured audit finding for a symbol",
    ),
    (
        "aether_audit_report",
        "Query audit findings by crate, severity, category, or status",
    ),
    (
        "aether_audit_resolve",
        "Mark an audit finding as fixed, wontfix, or confirmed",
    ),
    (
        "aether_contract_add",
        "add a behavioral contract on a symbol",
    ),
    ("aether_contract_list", "list active intent contracts"),
    ("aether_contract_remove", "deactivate an intent contract"),
    (
        "aether_contract_check",
        "verify contracts against current SIR",
    ),
    (
        "aether_contract_violations",
        "query contract violation history",
    ),
    ("aether_contract_dismiss", "dismiss a contract violation"),
    (
        "aether_sir_inject",
        "Inject or update a symbol's complete SIR annotation, including intent, behavior, edge_cases, side_effects, dependencies, error_modes, inputs, outputs, complexity, confidence, and model provenance",
    ),
    (
        "aether_sir_context",
        "Assemble token-budgeted context for a symbol including source, SIR, graph neighbors, health, reasoning trace, and test intents in one call",
    ),
    (
        "aether_enhance_prompt",
        "Enhance a raw coding prompt with indexed codebase context, symbol matches, files, and architectural notes",
    ),
    (
        "aether_blast_radius",
        "Analyze coupled files and risk levels for blast-radius impact",
    ),
    (
        "aether_test_intents",
        "Query extracted behavioral test intents for a file or symbol",
    ),
    (
        "aether_drift_report",
        "Run semantic drift analysis with boundary and structural anomaly detection",
    ),
    (
        "aether_health",
        "Get codebase health metrics including critical symbols, bottlenecks, dependency cycles, orphaned code, and risk hotspots.",
    ),
    (
        "aether_health_hotspots",
        "Return the hottest workspace crates by health score with archetypes and top violations.",
    ),
    (
        "aether_health_explain",
        "Explain one crate's health score, signals, violations, and split suggestions.",
    ),
    (
        "aether_refactor_prep",
        "Prepare a file or crate for refactoring by deep-scanning the highest-risk symbols and saving an intent snapshot",
    ),
    (
        "aether_verify_intent",
        "Compare current SIR against a saved refactor-prep snapshot and flag semantic drift",
    ),
    (
        "aether_trace_cause",
        "Trace likely upstream semantic causes of a downstream breakage",
    ),
    (
        "aether_acknowledge_drift",
        "Acknowledge drift findings and create a project note",
    ),
    (
        "aether_symbol_timeline",
        "Get ordered SIR timeline entries for a symbol",
    ),
    (
        "aether_why_changed",
        "Explain why a symbol changed between two SIR versions or timestamps",
    ),
    ("aether_get_sir", "Get SIR for leaf/file/module level"),
    (
        "aether_explain",
        "Explain symbol at a file position using local SIR",
    ),
    (
        "aether_verify",
        "Run allowlisted verification commands in host, container, or microvm mode",
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

pub(crate) fn required_actions() -> [&'static str; 5] {
    [
        "Always call `aether_get_sir` before reverting, deleting, or refactoring symbols.",
        "Always call `aether_why_changed` before reverting recent changes.",
        "Always call `aether_verify` after modifying code.",
        "If `aether_verify` fails, fix the issue before proceeding.",
        "After creating or significantly modifying key symbols, call `aether_sir_inject` with the full fields you know: intent, behavior, edge_cases, side_effects, dependencies, error_modes, inputs, outputs, complexity, confidence, and model.",
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
        TOOL_DESCRIPTIONS, TemplateContext,
    };

    fn sample_context() -> TemplateContext {
        TemplateContext {
            languages: vec!["Rust".to_owned(), "Python".to_owned()],
            verify_commands: vec![
                "cargo fmt --all --check".to_owned(),
                "cargo clippy --workspace -- -D warnings".to_owned(),
                "cargo test --workspace".to_owned(),
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
        assert!(rendered.contains("cargo fmt --all --check"));
        assert!(rendered.contains("cargo clippy --workspace -- -D warnings"));
        assert!(rendered.contains("cargo test --workspace"));
        assert!(rendered.contains("aether_get_sir"));
        assert!(rendered.contains("aether_audit_submit"));
        assert!(rendered.contains("aether_sir_inject"));
        assert!(rendered.contains("aether_sir_context"));
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

    #[test]
    fn tool_descriptions_include_enhance_prompt_tool() {
        assert_eq!(TOOL_DESCRIPTIONS.len(), 40);
        assert!(
            TOOL_DESCRIPTIONS
                .iter()
                .any(|(name, _)| *name == "aether_enhance_prompt")
        );
    }
}
