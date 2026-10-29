//! The files `aetherd init-agent` writes name MCP tools in prose, so a renamed, removed
//! or feature-gated tool would go unnoticed there. Every `aether_*` name they mention
//! must be a tool this server registers, built with the same features.

use std::collections::BTreeSet;

use aetherd::templates::{
    AuditChangesCommandTemplate, AuditCommandTemplate, AuditReportCommandTemplate, ClaudeTemplate,
    CodexInstructionsTemplate, ContextCommandTemplate, CursorRulesTemplate, EnrichCommandTemplate,
    ExplainAreaCommandTemplate, HealthCheckCommandTemplate, ImpactCommandTemplate,
    NextCommandTemplate, OmpAgentsTemplate, PreFlightCommandTemplate, RefactorCommandTemplate,
    RefactorDeepCommandTemplate, ReviewCommandTemplate, ScanAllScriptTemplate, ScanCommandTemplate,
    SkillTemplate, TemplateContext,
};
use tempfile::tempdir;

use super::AetherMcpServer;

fn registered_tools() -> BTreeSet<String> {
    let temp = tempdir().expect("tempdir");
    let server = AetherMcpServer::new(temp.path(), false).expect("server");
    server
        .tool_router
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

fn context() -> TemplateContext {
    TemplateContext {
        languages: vec!["Rust".to_owned()],
        source_suffixes: vec![".rs".to_owned()],
        verify_commands: vec!["cargo test".to_owned()],
        embeddings_enabled: true,
        inference_provider: "mock".to_owned(),
        agent_schema_version: 1,
        mcp_binary_hint: "./target/debug/aether-mcp".to_owned(),
        // What `init-agent` sets from the same build's features.
        verification_tool: cfg!(feature = "verification"),
        mcp_tools_arg: None,
    }
}

fn mentioned_tools(rendered: &str) -> BTreeSet<String> {
    rendered
        .match_indices("aether_")
        .filter(|(start, _)| {
            // Part of a longer identifier (`run_aether_x`) or the `aether_*` wildcard
            // `/context` uses to ban tool names.
            let previous = rendered[..*start].chars().next_back();
            !previous.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                && !rendered[*start..].starts_with("aether_*")
        })
        .map(|(start, _)| {
            rendered[start..]
                .chars()
                .take_while(|ch| ch.is_ascii_lowercase() || *ch == '_')
                .collect::<String>()
        })
        .collect()
}

#[test]
fn generated_agent_files_name_only_registered_tools() {
    let registered = registered_tools();
    let context = context();
    for (file, rendered) in [
        ("CLAUDE.md", ClaudeTemplate::render(&context)),
        ("AGENTS.md", OmpAgentsTemplate::render(&context)),
        (
            ".codex-instructions",
            CodexInstructionsTemplate::render(&context),
        ),
        (".cursor/rules", CursorRulesTemplate::render(&context)),
        ("SKILL.md", SkillTemplate::render(&context)),
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
        ("scan_all.sh", ScanAllScriptTemplate::render(&context)),
    ] {
        for tool in mentioned_tools(&rendered) {
            assert!(
                registered.contains(&tool),
                "{file} names {tool}, which this build does not register"
            );
        }
    }
}

/// The default build has no `aether_verify`, so the generated files must not ask for it.
#[cfg(not(feature = "verification"))]
#[test]
fn default_build_registers_no_verify_tool_and_agent_files_do_not_mandate_it() {
    assert!(!registered_tools().contains("aether_verify"));
    let context = context();
    for rendered in [
        ClaudeTemplate::render(&context),
        CodexInstructionsTemplate::render(&context),
        CursorRulesTemplate::render(&context),
        SkillTemplate::render(&context),
    ] {
        assert!(!mentioned_tools(&rendered).contains("aether_verify"));
    }
}
