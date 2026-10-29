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
fn review_regression_custom_selected_guidance_is_never_overwritten() {
    for name in [
        "CLAUDE.md",
        "AGENTS.md",
        ".codex-instructions",
        ".cursor/rules",
    ] {
        let temp = workspace();
        run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
        let registration = fs::read(temp.path().join(".mcp.json")).expect("registration");
        let custom = b"User-authored instructions. Preserve verbatim.\n";
        fs::write(temp.path().join(name), custom).expect("custom guidance");
        let result = run_init_agent(temp.path(), options(McpToolProfile::Compact, true));
        assert!(
            result.is_err(),
            "custom {name} must be refused before transition"
        );
        assert_eq!(fs::read(temp.path().join(name)).expect("guidance"), custom);
        assert_eq!(
            fs::read(temp.path().join(".mcp.json")).expect("registration"),
            registration
        );
        assert!(temp.path().join("scripts/scan_all.sh").exists());
    }
}

#[test]
fn review_regression_compact_to_full_requires_force_before_any_write() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Compact, false)).expect("compact setup");
    let registration = fs::read(temp.path().join(".mcp.json")).expect("registration");
    assert!(run_init_agent(temp.path(), options(McpToolProfile::Full, false)).is_err());
    assert!(!temp.path().join("scripts/scan_all.sh").exists());
    assert_eq!(
        fs::read(temp.path().join(".mcp.json")).expect("registration"),
        registration
    );
}

#[test]
fn review_regression_mcp_custom_fields_and_extra_arguments_survive() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let path = temp.path().join(".mcp.json");
    let mut registration: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("registration")).expect("JSON");
    registration["mcpServers"]["aether"]["env"] = serde_json::json!({"FIXTURE_SETTING":"keep"});
    registration["mcpServers"]["aether"]["args"]
        .as_array_mut()
        .expect("arguments")
        .extend([
            serde_json::json!("--fixture-option"),
            serde_json::json!("keep"),
        ]);
    fs::write(&path, serde_json::to_vec(&registration).expect("encode"))
        .expect("custom registration");
    run_init_agent(temp.path(), options(McpToolProfile::Compact, true))
        .expect("compact transition");
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("registration")).expect("JSON");
    assert_eq!(
        after["mcpServers"]["aether"]["env"],
        registration["mcpServers"]["aether"]["env"]
    );
    assert_eq!(
        after["mcpServers"]["aether"]["args"],
        serde_json::json!([
            "--workspace",
            ".",
            "--tools",
            "compact",
            "--fixture-option",
            "keep"
        ])
    );
}

#[test]
fn review_regression_custom_compact_profile_is_detected_from_shared_registration() {
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Compact, false)).expect("compact setup");
    for name in [
        "CLAUDE.md",
        "AGENTS.md",
        ".codex-instructions",
        ".cursor/rules",
    ] {
        fs::write(temp.path().join(name), "Custom instructions; preserve.\n")
            .expect("custom guidance");
    }
    let path = temp.path().join(".mcp.json");
    let mut registration: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("registration")).expect("JSON");
    registration["mcpServers"]["other"] = serde_json::json!({"command":"other"});
    fs::write(&path, serde_json::to_vec(&registration).expect("encode"))
        .expect("shared registration");
    assert!(run_init_agent(temp.path(), options(McpToolProfile::Full, true)).is_err());
    assert_eq!(
        fs::read_to_string(temp.path().join("CLAUDE.md")).expect("guidance"),
        "Custom instructions; preserve.\n"
    );
}

#[cfg(unix)]
#[test]
fn review_regression_partial_transition_has_recovery_manifest_and_can_replay() {
    use std::os::unix::fs::PermissionsExt;
    let temp = workspace();
    run_init_agent(temp.path(), options(McpToolProfile::Full, false)).expect("full setup");
    let cursor = temp.path().join(".cursor");
    let permissions = fs::metadata(&cursor)
        .expect("cursor metadata")
        .permissions();
    fs::set_permissions(&cursor, fs::Permissions::from_mode(0o555))
        .expect("deny rename for fixture");
    let result = run_init_agent(temp.path(), options(McpToolProfile::Compact, true));
    fs::set_permissions(&cursor, permissions).expect("restore fixture permissions");
    let error = format!("{:#}", result.expect_err("fixture rename must fail"));
    assert!(error.contains("recovery.json"), "{error}");
    let backup = fs::read_dir(temp.path().join(".aether/agent-artifact-backups"))
        .expect("backup root")
        .next()
        .expect("backup entry")
        .expect("backup directory")
        .path();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(backup.join("recovery.json")).expect("durable manifest"))
            .expect("manifest JSON");
    for entry in manifest["artifacts"].as_array().expect("inventory") {
        let original = temp
            .path()
            .join(entry["original_path"].as_str().expect("original"));
        let archived = temp
            .path()
            .join(entry["archive_path"].as_str().expect("archive"));
        if archived.exists() {
            assert!(
                !original.exists(),
                "recovery must never overwrite active content"
            );
            fs::rename(archived, original).expect("reviewed fixture recovery");
        }
    }
    let compact = run_init_agent(temp.path(), options(McpToolProfile::Compact, true))
        .expect("retry after recovery");
    assert!(!compact.archived_files.is_empty());
    assert!(
        run_init_agent(temp.path(), options(McpToolProfile::Compact, true))
            .expect("replay")
            .archived_files
            .is_empty()
    );
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
    let all_guidance = full.written_files.clone();
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
    let original_bytes = all_guidance
        .iter()
        .filter(|path| path.as_path() != Path::new(".mcp.json"))
        .map(|path| {
            (
                path.clone(),
                fs::read(temp.path().join(path)).expect("original bytes"),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let compact = run_init_agent(temp.path(), options(McpToolProfile::Compact, true))
        .expect("compact transition");
    assert_eq!(compact.archived_files.len(), original_bytes.len());
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
