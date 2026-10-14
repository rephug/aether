pub mod audit_changes_cmd;
pub mod audit_cmd;
pub mod audit_report_cmd;
pub mod claude_md;
pub mod codex_instructions;
pub mod cursor_rules;
pub mod enrich_cmd;
pub mod explain_area_cmd;
pub mod impact_cmd;
pub mod next_cmd;
pub mod omp;
pub mod pre_flight_cmd;
pub mod refactor_cmd;
pub mod refactor_deep_cmd;
pub mod review_cmd;
pub mod scan_all_script;
pub mod scan_cmd;
pub mod skill_md;

pub use audit_changes_cmd::AuditChangesCommandTemplate;
pub use audit_cmd::AuditCommandTemplate;
pub use audit_report_cmd::AuditReportCommandTemplate;
pub use claude_md::ClaudeTemplate;
pub use codex_instructions::CodexInstructionsTemplate;
pub use cursor_rules::CursorRulesTemplate;
pub use enrich_cmd::EnrichCommandTemplate;
pub use explain_area_cmd::ExplainAreaCommandTemplate;
pub use impact_cmd::ImpactCommandTemplate;
pub use next_cmd::NextCommandTemplate;
pub use omp::{McpJsonTemplate, OmpAgentsTemplate};
pub use pre_flight_cmd::PreFlightCommandTemplate;
pub use refactor_cmd::RefactorCommandTemplate;
pub use refactor_deep_cmd::RefactorDeepCommandTemplate;
pub use review_cmd::ReviewCommandTemplate;
pub use scan_all_script::ScanAllScriptTemplate;
pub use scan_cmd::ScanCommandTemplate;
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
        AuditChangesCommandTemplate, AuditCommandTemplate, AuditReportCommandTemplate,
        ClaudeTemplate, CodexInstructionsTemplate, CursorRulesTemplate, EnrichCommandTemplate,
        ExplainAreaCommandTemplate, ImpactCommandTemplate, McpJsonTemplate, NextCommandTemplate,
        OmpAgentsTemplate, PreFlightCommandTemplate, RefactorCommandTemplate,
        RefactorDeepCommandTemplate, ReviewCommandTemplate, ScanAllScriptTemplate,
        ScanCommandTemplate, SkillTemplate, TOOL_DESCRIPTIONS, TemplateContext,
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
        assert!(rendered.contains("## Zero-Key Onboarding"));
        assert!(!rendered.contains("recommended path"));
        assert!(rendered.contains("--inference-provider mock"));
        assert!(rendered.contains("./scripts/scan_all.sh"));
    }

    #[test]
    fn claude_template_lists_slash_commands_and_workflow() {
        let rendered = ClaudeTemplate::render(&sample_context());
        assert!(rendered.contains("\n\n## Slash Commands\n"));
        assert!(rendered.contains(
            "Recommended workflow: `/explain-area` for an unfamiliar area, then `/pre-flight <task>`, implement, then `/review`."
        ));
        for command in ["/pre-flight", "/review", "/explain-area", "/impact"] {
            assert!(rendered.contains(&format!("- `{command} ")), "{command}");
        }
        assert!(rendered.ends_with("before changing a widely used symbol.\n"));
    }

    fn command_templates(context: &TemplateContext) -> Vec<(&'static str, String)> {
        vec![
            ("audit", AuditCommandTemplate::render(context)),
            (
                "audit-changes",
                AuditChangesCommandTemplate::render(context),
            ),
            ("audit-report", AuditReportCommandTemplate::render(context)),
            ("enrich", EnrichCommandTemplate::render(context)),
            ("explain-area", ExplainAreaCommandTemplate::render(context)),
            ("impact", ImpactCommandTemplate::render(context)),
            ("next", NextCommandTemplate::render(context)),
            ("pre-flight", PreFlightCommandTemplate::render(context)),
            ("refactor", RefactorCommandTemplate::render(context)),
            (
                "refactor-deep",
                RefactorDeepCommandTemplate::render(context),
            ),
            ("review", ReviewCommandTemplate::render(context)),
            ("scan", ScanCommandTemplate::render(context)),
        ]
    }

    /// Slash commands are static text, so a renamed or removed tool would go unnoticed:
    /// every `aether_*` name they mention must be in `TOOL_DESCRIPTIONS`, the tool list
    /// the generated CLAUDE.md is built from.
    #[test]
    fn command_templates_name_only_registered_tools() {
        for (command, rendered) in command_templates(&sample_context()) {
            for (start, _) in rendered.match_indices("aether_") {
                let name: String = rendered[start..]
                    .chars()
                    .take_while(|ch| ch.is_ascii_lowercase() || *ch == '_')
                    .collect();
                assert!(
                    TOOL_DESCRIPTIONS.iter().any(|(tool, _)| *tool == name),
                    "/{command} names unknown tool {name}"
                );
            }
        }
    }

    #[test]
    fn workflow_command_templates_render_front_matter_and_steps() {
        let context = sample_context();
        let pre_flight = PreFlightCommandTemplate::render(&context);
        assert!(pre_flight.starts_with("---\nargument-hint: <task description>\ndescription: "));
        for tool in [
            "aether_search",
            "aether_sir_context",
            "aether_get_sir",
            "aether_dependencies",
            "aether_usage_matrix",
            "aether_health_hotspots",
            "aether_health_explain",
            "aether_contract_list",
        ] {
            assert!(
                pre_flight.contains(&format!("`{tool}`")),
                "/pre-flight {tool}"
            );
        }
        assert!(pre_flight.contains("**Recommended approach:**"));

        let review = ReviewCommandTemplate::render(&context);
        assert!(review.starts_with("---\nargument-hint: [base-branch]\ndescription: "));
        assert!(review.contains("git diff <base>...HEAD"));
        for tool in [
            "aether_status",
            "aether_symbol_lookup",
            "aether_get_sir",
            "aether_why_changed",
            "aether_dependencies",
            "aether_contract_list",
            "aether_contract_check",
            "aether_verify_intent",
            "aether_sir_inject",
        ] {
            assert!(review.contains(&format!("`{tool}`")), "/review {tool}");
        }
        assert!(review.contains("Only after the user agrees, call"));
        assert!(review.contains("`source_hash`: the hash from Step 2"));

        let explain = ExplainAreaCommandTemplate::render(&context);
        assert!(
            explain.starts_with(
                "---\nargument-hint: <file, directory, crate or symbol>\ndescription: "
            )
        );
        assert!(explain.contains("`level` set to `module`"));
        assert!(explain.contains("`aether_call_chain`"));

        let impact = ImpactCommandTemplate::render(&context);
        assert!(impact.starts_with("---\nargument-hint: <symbol>\ndescription: "));
        for tool in [
            "aether_blast_radius",
            "aether_dependencies",
            "aether_test_intents",
        ] {
            assert!(impact.contains(&format!("`{tool}`")), "/impact {tool}");
        }
        assert!(impact.contains("**High:**"));

        for rendered in [&pre_flight, &review, &explain, &impact] {
            assert!(rendered.contains("$ARGUMENTS"));
            assert!(rendered.ends_with('\n'));
        }
        // Three commands never write; /review writes only what the user approves and, like
        // the others, never asks for a refactor-prep deep scan.
        for rendered in [&pre_flight, &explain, &impact] {
            assert!(rendered.contains("doesn't edit files or inject SIRs"));
        }
        assert!(!review.contains("`aether_refactor_prep`"));
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
    fn scan_templates_render_command_and_script() {
        let context = sample_context();
        let command = ScanCommandTemplate::render(&context);
        assert!(command.contains("/scan <crate> [batch-size]"));
        assert!(command.contains("aether_sir_inject"));
        assert!(command.contains("`batch-size` as `top_n`"));
        let script = ScanAllScriptTemplate::render(&context);
        assert!(script.starts_with("#!/usr/bin/env bash"));
        assert!(script.contains("cargo metadata --no-deps"));
        assert!(!script.contains("pkill"));
        assert!(!script.contains("declare -A"));
        assert!(!script.contains("mapfile"));
        assert!(!script.contains("wait -n"));
        assert!(!script.contains("git rev-parse"));
        assert!(script.contains(".mcp.json"));
        assert!(script.contains("FROM symbols"));
        assert!(script.contains("--inference-provider mock"));
        assert!(!script.contains("enrich_all.sh"));
    }

    /// AETHER dogfoods its own generated assets: the checked-in copies must match the
    /// templates byte for byte. The slash commands are checked below.
    #[test]
    fn checked_in_scan_script_matches_template() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root");
        let context = sample_context();
        let script = std::fs::read_to_string(root.join("scripts/scan_all.sh"))
            .expect("read scripts/scan_all.sh");
        assert_eq!(script, ScanAllScriptTemplate::render(&context));
    }

    /// Every generated slash command is tracked in `.claude/commands/`, and each copy
    /// must match its template byte for byte.
    #[test]
    fn checked_in_commands_match_templates() {
        let commands = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root")
            .join(".claude/commands");
        let context = sample_context();
        for (file, rendered) in [
            ("audit.md", AuditCommandTemplate::render(&context)),
            (
                "audit-changes.md",
                AuditChangesCommandTemplate::render(&context),
            ),
            (
                "audit-report.md",
                AuditReportCommandTemplate::render(&context),
            ),
            ("enrich.md", EnrichCommandTemplate::render(&context)),
            (
                "explain-area.md",
                ExplainAreaCommandTemplate::render(&context),
            ),
            ("impact.md", ImpactCommandTemplate::render(&context)),
            ("next.md", NextCommandTemplate::render(&context)),
            ("pre-flight.md", PreFlightCommandTemplate::render(&context)),
            ("refactor.md", RefactorCommandTemplate::render(&context)),
            (
                "refactor-deep.md",
                RefactorDeepCommandTemplate::render(&context),
            ),
            ("review.md", ReviewCommandTemplate::render(&context)),
            ("scan.md", ScanCommandTemplate::render(&context)),
        ] {
            let checked_in = std::fs::read_to_string(commands.join(file))
                .unwrap_or_else(|err| panic!("read .claude/commands/{file}: {err}"));
            assert_eq!(
                checked_in, rendered,
                ".claude/commands/{file} is out of date"
            );
        }
    }

    #[test]
    fn omp_agents_template_matches_claude_guidance() {
        let context = sample_context();
        assert_eq!(
            OmpAgentsTemplate::render(&context),
            ClaudeTemplate::render(&context)
        );
    }

    #[test]
    fn mcp_json_template_registers_aether_server() {
        let rendered = McpJsonTemplate::render(&sample_context());
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
        assert_eq!(
            parsed["mcpServers"]["aether"]["command"],
            "./target/debug/aether-mcp"
        );
        assert_eq!(parsed["mcpServers"]["aether"]["args"][0], "--workspace");
        assert!(rendered.ends_with('\n'));
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
