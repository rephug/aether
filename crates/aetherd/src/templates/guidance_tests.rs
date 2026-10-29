use super::{
    ClaudeTemplate, CodexInstructionsTemplate, CursorRulesTemplate, OmpAgentsTemplate,
    ScanCommandTemplate, SkillTemplate, TemplateContext, required_actions, verification_guidance,
};

fn context(commands: &[&str]) -> TemplateContext {
    TemplateContext {
        languages: vec!["Rust".to_owned()],
        source_suffixes: vec![".rs".to_owned()],
        verify_commands: commands
            .iter()
            .map(|command| (*command).to_owned())
            .collect(),
        embeddings_enabled: false,
        inference_provider: "mock".to_owned(),
        agent_schema_version: 7,
        mcp_binary_hint: "aether-mcp".to_owned(),
        verification_tool: false,
        mcp_tools_arg: None,
    }
}

fn shared_guidance(context: &TemplateContext) -> [(&'static str, String); 5] {
    [
        ("Claude", ClaudeTemplate::render(context)),
        ("Codex", CodexInstructionsTemplate::render(context)),
        ("Cursor", CursorRulesTemplate::render(context)),
        ("omp", OmpAgentsTemplate::render(context)),
        ("Skill", SkillTemplate::render(context)),
    ]
}

#[test]
fn shared_clients_require_repository_validation_without_mandating_optional_tools() {
    for commands in [vec![], vec!["fixture-check"]] {
        let context = context(&commands);
        for required in required_actions(&context) {
            assert!(!required.contains("aether_get_sir"));
            assert!(!required.contains("aether_why_changed"));
            assert!(!required.contains("aether_sir_inject"));
        }
        for (name, guide) in shared_guidance(&context) {
            let normalized = guide.replace('`', "").to_lowercase();
            assert!(
                guide.contains("Run relevant repository validation and regression tests"),
                "{name}"
            );
            assert!(
                guide.contains("Ordinary repository exploration remains available"),
                "{name}"
            );
            for stale_directive in [
                "always call aether_",
                "after code changes, call aether_verify",
                "after modifications: call aether_verify",
                "run aether_verify before completing",
                "before editing or refactoring symbols, call aether_get_sir",
                "confirm index freshness",
            ] {
                assert!(
                    !normalized.contains(stale_directive),
                    "{name}: {stale_directive}"
                );
            }
            assert!(guide.contains("Counts cannot prove freshness, dependency completeness or successful synchronization"), "{name}");
            assert!(guide.contains("If `aether_get_sir` is available"), "{name}");
            assert!(
                guide.contains("If `aether_why_changed` and retained versions are available"),
                "{name}"
            );
        }
    }
}

#[test]
fn verify_workflow_requires_configured_commands_and_advertised_feature() {
    let empty = context(&[]);
    let empty_guidance = verification_guidance(&empty);
    assert!(!empty_guidance.contains("aether_verify"));
    assert!(empty_guidance.contains("No AETHER verify commands are configured"));
    for (name, guide) in shared_guidance(&empty) {
        assert!(
            guide.contains(empty_guidance.trim_end_matches('.')),
            "{name}"
        );
    }
    let configured = TemplateContext {
        verification_tool: true,
        ..context(&["fixture-check", "fixture-regression-check"])
    };
    let guidance = verification_guidance(&configured);
    assert!(guidance.contains("If the connected server advertises `aether_verify`"));
    assert!(guidance.contains("verification feature and these configured commands"));
    assert!(guidance.contains("you may use it; otherwise run the commands directly"));
    for (name, guide) in shared_guidance(&configured) {
        assert!(guide.contains(guidance.trim_end_matches('.')), "{name}");
        assert!(guide.contains("fixture-check"), "{name}");
        assert!(guide.contains("fixture-regression-check"), "{name}");
    }
}

#[test]
fn shared_annotation_writes_require_explicit_workflow_and_writable_tools() {
    let context = context(&[]);
    for (name, guide) in shared_guidance(&context) {
        assert!(
            guide.contains(
                "When explicitly asked to record SIR annotations, and writable tools are available"
            ),
            "{name}"
        );
        assert!(guide.contains("It writes the shared store"), "{name}");
        assert!(
            guide.contains("use a read-only connection for read-only work"),
            "{name}"
        );
    }
    // Opt-in slash commands retain their intentional write workflow.
    assert!(ScanCommandTemplate::render(&context).contains("aether_sir_inject"));
}

#[test]
fn unavailable_verification_tool_is_not_advertised() {
    let configured = context(&["fixture-check"]);
    for (name, guide) in shared_guidance(&configured) {
        let mentions_verify = guide
            .split(|character: char| !character.is_ascii_lowercase() && character != '_')
            .any(|tool| tool == "aether_verify");
        assert!(!mentions_verify, "{name}");
        assert!(guide.contains("fixture-check"), "{name}");
    }
}
