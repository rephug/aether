pub mod audit_changes_cmd;
pub mod audit_cmd;
pub mod audit_report_cmd;
pub mod ci_review_workflow;
pub mod claude_md;
pub mod codex_instructions;
pub mod context_cmd;
pub mod cursor_rules;
pub mod enrich_cmd;
pub mod explain_area_cmd;
pub mod health_check_cmd;
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
pub use ci_review_workflow::GithubReviewWorkflowTemplate;
pub use claude_md::ClaudeTemplate;
pub use codex_instructions::CodexInstructionsTemplate;
pub use context_cmd::ContextCommandTemplate;
pub use cursor_rules::CursorRulesTemplate;
pub use enrich_cmd::EnrichCommandTemplate;
pub use explain_area_cmd::ExplainAreaCommandTemplate;
pub use health_check_cmd::HealthCheckCommandTemplate;
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
    /// File suffixes (`.rs`, `.py`, ...) the registered parsers handle.
    pub source_suffixes: Vec<String>,
    pub verify_commands: Vec<String>,
    pub embeddings_enabled: bool,
    pub inference_provider: String,
    pub agent_schema_version: u32,
    pub mcp_binary_hint: String,
}

pub(crate) const TOOL_DESCRIPTIONS: [(&str, &str); 41] = [
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
        "aether_preflight",
        "Assemble a deterministic provider-free task map with dependencies, reverse impact, recorded history, risks and tests",
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
        ClaudeTemplate, CodexInstructionsTemplate, ContextCommandTemplate, CursorRulesTemplate,
        EnrichCommandTemplate, ExplainAreaCommandTemplate, GithubReviewWorkflowTemplate,
        HealthCheckCommandTemplate, ImpactCommandTemplate, McpJsonTemplate, NextCommandTemplate,
        OmpAgentsTemplate, PreFlightCommandTemplate, RefactorCommandTemplate,
        RefactorDeepCommandTemplate, ReviewCommandTemplate, ScanAllScriptTemplate,
        ScanCommandTemplate, SkillTemplate, TOOL_DESCRIPTIONS, TemplateContext,
    };

    fn sample_context() -> TemplateContext {
        TemplateContext {
            languages: vec!["Rust".to_owned(), "Python".to_owned()],
            source_suffixes: vec![".py".to_owned(), ".pyi".to_owned(), ".rs".to_owned()],
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
        assert!(rendered.contains("run `aetherd --workspace . --index-once`"));
        assert!(!rendered.contains("aether_index_once"));
    }

    #[test]
    fn claude_template_lists_slash_commands_and_workflow() {
        let rendered = ClaudeTemplate::render(&sample_context());
        assert!(rendered.contains("\n\n## Slash Commands\n"));
        assert!(rendered.contains(
            "Recommended workflow: `/explain-area` for an unfamiliar area, then `/pre-flight <task>`, implement, then `/review`."
        ));
        for command in [
            "/pre-flight",
            "/review",
            "/explain-area",
            "/impact",
            "/context",
            "/health-check`",
        ] {
            assert!(rendered.contains(&format!("- `{command}")), "{command}");
        }
        assert!(rendered.ends_with("and `/health-check` at the start of a session.\n"));
    }

    fn command_templates(context: &TemplateContext) -> Vec<(&'static str, String)> {
        vec![
            ("audit", AuditCommandTemplate::render(context)),
            (
                "audit-changes",
                AuditChangesCommandTemplate::render(context),
            ),
            ("audit-report", AuditReportCommandTemplate::render(context)),
            ("context", ContextCommandTemplate::render(context)),
            ("enrich", EnrichCommandTemplate::render(context)),
            ("explain-area", ExplainAreaCommandTemplate::render(context)),
            ("health-check", HealthCheckCommandTemplate::render(context)),
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
                if rendered[start..].starts_with("aether_*") {
                    // The wildcard /context uses to ban tool names.
                    continue;
                }
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
        assert!(pre_flight.contains("`aether_preflight`"));
        assert!(pre_flight.contains("Read the returned `map.context` before exploring files"));
        assert!(pre_flight.contains("source-only"));
        assert!(pre_flight.contains("requires no branch name"));
        assert!(!pre_flight.contains("`aether_search`"));
        assert!(!pre_flight.contains("`aether_sir_context`"));
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
    fn preflight_command_receives_one_map_before_exploration() {
        let command = PreFlightCommandTemplate::render(&sample_context());
        let assembled = command
            .find("Call `aether_preflight` first")
            .expect("assembled map");
        let explore = command
            .find("Use the map's investigation order")
            .expect("exploration");
        assert!(assembled < explore);
        assert!(command.contains("without initializing a store"));
        assert!(command.contains("must not run enrichment or make model/provider calls"));
        assert!(command.contains("continue\nordinary repository exploration"));
        assert!(command.contains("Do not initialize, reindex or enrich as a fallback"));
    }

    #[test]
    fn context_template_keeps_its_output_free_of_aether_terms() {
        let context = ContextCommandTemplate::render(&sample_context());
        assert!(
            context.starts_with(
                "---\nargument-hint: <file, directory, crate or symbol>\ndescription: "
            )
        );
        assert!(context.contains("$ARGUMENTS"));
        assert!(context.contains("doesn't edit files or inject SIRs"));
        for tool in [
            "aether_get_sir",
            "aether_sir_context",
            "aether_call_chain",
            "aether_dependencies",
            "aether_test_intents",
        ] {
            assert!(context.contains(&format!("`{tool}`")), "/context {tool}");
        }
        // The document it asks for must stand alone and avoid AETHER vocabulary.
        let rules = context
            .split("## Rules for the document")
            .nth(1)
            .expect("rules section");
        assert!(rules.contains("never refers to tools, slash commands, this conversation"));
        for banned in [
            "\"SIR\"",
            "\"symbol id\"",
            "\"confidence\"",
            "\"generation pass\"",
            "\"health score\"",
            "\"blast radius\"",
            "\"AETHER\"",
            "`aether_*` names",
        ] {
            assert!(rules.contains(banned), "/context rules must ban {banned}");
        }
        // A banned word that names the project or an identifier stays as the code has it.
        assert!(rules.contains("the project's own name or an identifier in its code"));
        assert!(context.contains("four backticks"));
    }

    #[test]
    fn review_workflow_is_advisory_keyless_and_sticky() {
        let workflow = GithubReviewWorkflowTemplate::render(&sample_context());
        assert!(workflow.starts_with("# Advisory AETHER review"));
        assert!(workflow.contains("don't make it a required check"));
        assert!(workflow.contains("\non:\n  pull_request:\n"));
        assert!(!workflow.contains("pull_request_target"));
        assert!(workflow.contains("permissions:\n  contents: read\n  pull-requests: write\n"));
        assert!(workflow.contains("    continue-on-error: true\n"));
        // Queued, not cancelled, so a first build is never thrown away mid-way.
        assert!(workflow.contains("cancel-in-progress: false"));
        // pipefail, so a failed listing or ls-remote fails its step instead of
        // silently falling back.
        assert!(workflow.contains("defaults:\n  run:\n    shell: bash\n"));
        // The checkout must not leave the job token where the built tool can read it.
        assert!(workflow.contains("persist-credentials: false"));

        // aetherd: resolved strictly, restored from any earlier build, built only when
        // needed, and saved as soon as it is built.
        assert!(workflow.contains("\"refs/heads/$AETHER_REF\""));
        assert!(workflow.contains("is not a branch, tag or commit"));
        assert!(workflow.contains("uses: actions/cache/restore@v4"));
        assert!(workflow.contains("restore-keys: aetherd-${{ runner.os }}-\n"));
        assert!(workflow.contains("steps.restore.outputs.cache-matched-key == ''"));
        assert!(workflow.contains(
            "cargo +stable install --locked --git \"$AETHER_GIT_URL\" --rev \"$AETHER_REV\""
        ));
        assert!(workflow.contains("uses: actions/cache/save@v4"));
        assert!(workflow.contains("if: steps.build.outcome == 'success'"));
        // Scheduled and manual runs seed the default branch's cache.
        assert!(workflow.contains("  schedule:\n"));
        assert!(workflow.contains("  workflow_dispatch:\n"));

        // Structural index only: no SIR generation, no secrets.
        assert!(
            workflow.contains("aetherd --workspace . --index-once --inference-provider mock\n")
        );
        assert!(!workflow.contains("--full"));
        assert!(!workflow.contains("secrets."));
        assert!(workflow.contains("GH_TOKEN: ${{ github.token }}"));

        // One comment, found by its marker and author and edited in place; fork PRs
        // (read-only token) get the job summary instead.
        assert!(workflow.contains("MARKER = \"<!-- aether-review -->\""));
        assert!(workflow.contains("marker='<!-- aether-review -->'"));
        assert!(workflow.contains("select(.user.login == \\\"github-actions[bot]\\\""));
        assert!(workflow.contains("gh api --method PATCH"));
        assert!(workflow.contains("gh pr comment \"$PR_NUMBER\" --body-file aether-review.md"));
        assert!(
            workflow.contains("github.event.pull_request.head.repo.full_name == github.repository")
        );
        assert!(workflow.contains("cat aether-review.md >> \"$GITHUB_STEP_SUMMARY\""));
        // A failed command must not read as a clean result.
        assert!(workflow.contains("Unavailable: `aetherd blast-radius` failed"));
        assert!(workflow.contains("Unavailable: `aetherd health` failed"));
        // Changed files are filtered on the context's suffixes, as one Python tuple.
        assert!(workflow.contains("\n          SOURCE_SUFFIXES = (\".py\", \".pyi\", \".rs\")\n"));
        assert!(workflow.contains("if path.endswith(SOURCE_SUFFIXES)]"));
        assert!(!workflow.contains("@SOURCE_SUFFIXES@"));

        for command in [
            "blast-radius",
            "\"health\"",
            "health-score",
            "mine-coupling",
        ] {
            assert!(workflow.contains(command), "{command}");
        }
        // health-score needs a Cargo workspace, not just any Cargo.toml.
        assert!(workflow.contains("^\\[workspace\\]"));
        assert!(workflow.ends_with("fi\n"));
        assert!(!workflow.contains('\t'), "YAML must not contain tabs");
    }

    #[test]
    fn health_check_template_caps_crates_and_recommendations() {
        let health = HealthCheckCommandTemplate::render(&sample_context());
        assert!(health.starts_with("---\ndescription: "));
        assert!(health.contains("doesn't edit files or inject SIRs"));
        for tool in ["aether_status", "aether_health_hotspots", "aether_health"] {
            assert!(
                health.contains(&format!("`{tool}`")),
                "/health-check {tool}"
            );
        }
        assert!(health.contains("If `symbol_count` is 0"));
        assert!(health.contains("`percentage` is a fraction"));
        assert!(health.contains("sqlite3 -readonly .aether/meta.sqlite"));
        assert!(health.contains("mode=ro"));
        assert!(health.contains("at most 5 crates"));
        assert!(health.contains("at most 3 recommendations"));
        assert!(health.contains("All healthy:"));
        // Suggesting a full index run could start paid SIR generation.
        assert!(!health.contains("--full"));
    }

    #[test]
    fn codex_template_renders_plain_text_instructions() {
        let rendered = CodexInstructionsTemplate::render(&sample_context());

        assert!(rendered.contains("AETHER CODE INTELLIGENCE INSTRUCTIONS"));
        assert!(rendered.contains("Agent schema version: 7"));
        assert!(rendered.contains("Inference provider: mock"));
        assert!(rendered.contains("semantic search is enabled"));
        assert!(rendered.contains("run aetherd --workspace . --index-once"));
        assert!(!rendered.contains("aether_index_once"));
    }

    #[test]
    fn cursor_template_uses_numbered_directives() {
        let rendered = CursorRulesTemplate::render(&sample_context());

        assert!(rendered.contains("1. Before editing or refactoring"));
        assert!(rendered.contains("Agent schema version: 7"));
        assert!(rendered.contains("aether_verify"));
        assert!(rendered.contains("run aetherd --workspace . --index-once"));
        assert!(!rendered.contains("aether_index_once"));
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
            ("context.md", ContextCommandTemplate::render(&context)),
            ("enrich.md", EnrichCommandTemplate::render(&context)),
            (
                "explain-area.md",
                ExplainAreaCommandTemplate::render(&context),
            ),
            (
                "health-check.md",
                HealthCheckCommandTemplate::render(&context),
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
        assert_eq!(TOOL_DESCRIPTIONS.len(), 41);
        assert!(
            TOOL_DESCRIPTIONS
                .iter()
                .any(|(name, _)| *name == "aether_enhance_prompt")
        );
    }
}
