use super::*;

fn workspace() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("workspace");
    fs::create_dir(temp.path().join(".aether")).expect("config directory");
    fs::write(temp.path().join(".aether/config.toml"),
        "[inference]\nprovider=\"mock\"\n[storage]\ngraph_backend=\"sqlite\"\n[embeddings]\nenabled=false\nvector_backend=\"sqlite\"\n[verify]\ncommands=[\"fixture-check\"]\n").expect("fixture config");
    temp
}

fn options(profile: McpToolProfile, force: bool) -> InitAgentOptions {
    InitAgentOptions {
        platform: AgentPlatform::All,
        force,
        mcp_tools: Some(profile),
    }
}

#[test]
fn full_to_compact_retires_full_only_artifacts_from_discovery() {
    let temp = workspace();
    let full =
        run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let core = [
        "CLAUDE.md",
        "AGENTS.md",
        ".codex-instructions",
        ".cursor/rules",
        ".mcp.json",
    ];
    let obsolete = full
        .written_files
        .into_iter()
        .filter(|path| !core.iter().any(|core| path == Path::new(core)))
        .collect::<Vec<_>>();
    assert!(
        obsolete
            .iter()
            .any(|path| path == Path::new("scripts/scan_all.sh"))
    );
    assert!(
        obsolete
            .iter()
            .any(|path| path == Path::new(".github/workflows/aether-review.yml"))
    );
    let original_bytes = obsolete
        .iter()
        .map(|path| {
            (
                path.clone(),
                fs::read(temp.path().join(path)).expect("original bytes"),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let compact = run_init_agent(temp.path(), options(McpToolProfile::Compact, true))
        .expect("compact transition");
    assert_eq!(compact.archived_files.len(), obsolete.len());
    for archived in &compact.archived_files {
        assert_eq!(
            fs::read(temp.path().join(&archived.archive_path)).expect("archived bytes"),
            original_bytes[&archived.original_path]
        );
    }
    for path in &obsolete {
        assert!(
            !temp.path().join(path).exists(),
            "full-only artifact remains active: {}",
            path.display()
        );
    }
    let registration: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join(".mcp.json")).expect("registration"))
            .expect("json");
    assert_eq!(
        registration["mcpServers"]["aether"]["args"],
        serde_json::json!(["--workspace", ".", "--tools", "compact"])
    );
}

#[test]
fn transition_without_force_refuses_before_registration_changes() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let registration = fs::read(temp.path().join(".mcp.json")).expect("registration");
    assert!(run_init_agent(temp.path(), options(McpToolProfile::Compact, false)).is_err());
    assert!(temp.path().join("scripts/scan_all.sh").exists());
    assert_eq!(
        fs::read(temp.path().join(".mcp.json")).expect("preserved registration"),
        registration
    );
}

#[test]
fn single_client_transition_refreshes_other_recognized_guidance() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let result = run_init_agent(
        temp.path(),
        InitAgentOptions {
            platform: AgentPlatform::Claude,
            force: true,
            mcp_tools: Some(McpToolProfile::Compact),
        },
    )
    .expect("compact transition");
    for path in ["AGENTS.md", ".codex-instructions", ".cursor/rules"] {
        let text = fs::read_to_string(temp.path().join(path)).expect("other client guidance");
        assert!(text.contains("compact tool profile"));
        assert!(
            result
                .archived_files
                .iter()
                .any(|entry| entry.original_path == Path::new(path))
        );
    }
    let repeat = run_init_agent(temp.path(), options(McpToolProfile::Compact, true))
        .expect("idempotent refresh");
    assert!(repeat.archived_files.is_empty());
}

#[test]
fn codex_transition_updates_existing_shared_registration_and_preserves_other_servers() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let path = temp.path().join(".mcp.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("registration")).expect("json");
    json["mcpServers"]["other"] =
        serde_json::json!({"command":"other-server", "args":["--unchanged"]});
    fs::write(&path, serde_json::to_vec(&json).expect("json bytes")).expect("shared config");
    run_init_agent(
        temp.path(),
        InitAgentOptions {
            platform: AgentPlatform::Codex,
            force: true,
            mcp_tools: Some(McpToolProfile::Compact),
        },
    )
    .expect("compact transition");
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("registration after")).expect("json after");
    assert_eq!(after["mcpServers"]["other"], json["mcpServers"]["other"]);
    assert_eq!(
        after["mcpServers"]["aether"]["args"],
        serde_json::json!(["--workspace", ".", "--tools", "compact"])
    );
}

#[test]
fn invalid_shared_registration_refuses_before_archival() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let path = temp.path().join(".mcp.json");
    fs::write(&path, "{custom invalid registration").expect("custom registration");
    assert!(run_init_agent(temp.path(), options(McpToolProfile::Compact, true)).is_err());
    assert!(temp.path().join("scripts/scan_all.sh").exists());
    assert!(!temp.path().join(".aether/agent-artifact-backups").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_full_artifact_parent_refuses_before_other_files_change() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let outside = tempfile::tempdir().expect("outside");
    fs::rename(temp.path().join("scripts"), outside.path().join("scripts")).expect("move scripts");
    std::os::unix::fs::symlink(outside.path().join("scripts"), temp.path().join("scripts"))
        .expect("symlink");
    let before = fs::read(temp.path().join(".mcp.json")).expect("registration");
    assert!(run_init_agent(temp.path(), options(McpToolProfile::Compact, true)).is_err());
    assert!(outside.path().join("scripts/scan_all.sh").exists());
    assert_eq!(
        fs::read(temp.path().join(".mcp.json")).expect("registration after"),
        before
    );
    assert!(temp.path().join(".claude/commands/scan.md").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_backup_directory_refuses_without_moving_artifacts() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(
        outside.path(),
        temp.path().join(".aether/agent-artifact-backups"),
    )
    .expect("backup symlink");
    assert!(run_init_agent(temp.path(), options(McpToolProfile::Compact, true)).is_err());
    assert!(temp.path().join("scripts/scan_all.sh").exists());
    assert!(
        fs::read_dir(outside.path())
            .expect("outside directory")
            .next()
            .is_none()
    );
}

#[test]
fn customized_full_artifact_refuses_compact_before_other_files_change() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let script = temp.path().join("scripts/scan_all.sh");
    let custom = b"#!/bin/sh\n# custom local work that must survive\n";
    fs::write(&script, custom).expect("customized artifact");
    let guidance = fs::read(temp.path().join("CLAUDE.md")).expect("guidance before");
    let registration = fs::read(temp.path().join(".mcp.json")).expect("registration before");
    let result = run_init_agent(temp.path(), options(McpToolProfile::Compact, true));
    assert!(
        result.is_err(),
        "unknown/custom artifacts must not be silently overwritten or left discoverable"
    );
    assert_eq!(fs::read(script).expect("preserved custom script"), custom);
    assert_eq!(
        fs::read(temp.path().join("CLAUDE.md")).expect("guidance after"),
        guidance
    );
    assert_eq!(
        fs::read(temp.path().join(".mcp.json")).expect("registration after"),
        registration
    );
}
