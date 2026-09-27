//! Refactor-prep / verify-intent round trip and the `aether_verify` runner modes.

use super::*;

#[test]
fn mcp_refactor_prep_and_verify_intent_tools_round_trip() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    write_test_config(workspace);
    fs::create_dir_all(workspace.join("src"))?;
    fs::write(
        workspace.join("Cargo.toml"),
        r#"[package]
name = "mcp-refactor-test"
version = "0.1.0"
edition = "2024"

[workspace]
members = ["."]
resolver = "2"
"#,
    )?;
    fs::write(
        workspace.join("src/lib.rs"),
        r#"pub fn alpha() -> i32 { 1 }
pub fn beta() -> i32 { alpha() }
"#,
    )?;
    run_index_and_seed_sir(workspace)?;
    mark_leaf_sir_deep(workspace)?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let prep = rt
        .block_on(
            server.aether_refactor_prep(Parameters(AetherRefactorPrepRequest {
                file: Some("src/lib.rs".to_owned()),
                crate_name: None,
                top_n: Some(2),
                local: Some(false),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(prep.schema_version, MCP_SCHEMA_VERSION);
    assert_eq!(prep.scope, "file:src/lib.rs");
    let snapshot_id = prep.snapshot_id.clone();
    assert_eq!(prep.deep_failed, 0);

    let verify = rt
        .block_on(
            server.aether_verify_intent(Parameters(AetherVerifyIntentRequest {
                snapshot: snapshot_id,
                threshold: Some(0.85),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(verify.schema_version, MCP_SCHEMA_VERSION);
    assert_eq!(verify.scope, "file:src/lib.rs");
    assert!(verify.passed);
    assert_eq!(verify.failed_entries, 0);
    Ok(())
}

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_runs_allowlisted_subset_and_has_stable_response_shape() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
commands = ["cargo --version", "cargo --definitely-invalid-flag"]
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let response = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: Some(vec!["cargo --version".to_owned()]),
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    assert_eq!(response.schema_version, MCP_SCHEMA_VERSION);
    assert_eq!(response.mode, "host");
    assert_eq!(response.mode_requested, "host");
    assert_eq!(response.mode_used, "host");
    assert_eq!(response.fallback_reason, None);
    assert_eq!(
        response.allowlisted_commands,
        vec![
            "cargo --version".to_owned(),
            "cargo --definitely-invalid-flag".to_owned()
        ]
    );
    assert_eq!(
        response.requested_commands,
        vec!["cargo --version".to_owned()]
    );
    assert!(response.passed);
    assert_eq!(response.error, None);
    assert_eq!(response.result_count, 1);
    assert_eq!(response.result_count as usize, response.results.len());
    assert_eq!(response.results[0].command, "cargo --version");
    assert_eq!(response.results[0].exit_code, Some(0));
    assert!(response.results[0].passed);
    assert!(response.results[0].stdout.contains("cargo"));

    let as_json = serde_json::to_value(&response)?;
    let object = as_json
        .as_object()
        .expect("verify response should serialize as object");
    for key in [
        "schema_version",
        "workspace",
        "mode",
        "mode_requested",
        "mode_used",
        "fallback_reason",
        "allowlisted_commands",
        "requested_commands",
        "passed",
        "error",
        "result_count",
        "results",
    ] {
        assert!(object.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_reports_failure_status_output_and_allowlist_rejection() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
commands = ["cargo --version", "cargo --definitely-invalid-flag"]
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let failure = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!failure.passed);
    assert_eq!(failure.error, None);
    assert_eq!(failure.result_count as usize, failure.results.len());
    assert_eq!(failure.results.len(), 2);
    assert_eq!(failure.results[0].command, "cargo --version");
    assert_eq!(failure.results[0].exit_code, Some(0));
    assert_eq!(
        failure.results[1].command,
        "cargo --definitely-invalid-flag"
    );
    assert_ne!(failure.results[1].exit_code, Some(0));
    assert!(!failure.results[1].passed);
    assert!(
        !failure.results[1].stderr.trim().is_empty()
            || !failure.results[1].stdout.trim().is_empty()
    );

    let rejected = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: Some(vec!["cargo --not-in-allowlist".to_owned()]),
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!rejected.passed);
    assert!(rejected.results.is_empty());
    assert_eq!(rejected.result_count, 0);
    assert_eq!(
        rejected.error.as_deref(),
        Some("requested command is not allowlisted: cargo --not-in-allowlist")
    );

    Ok(())
}

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_handles_unavailable_container_runtime_with_optional_fallback() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
mode = "container"
commands = ["cargo --version"]

[verify.container]
runtime = "definitely-missing-container-runtime"
image = "rust:1-bookworm"
workdir = "/workspace"
fallback_to_host_on_unavailable = false
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let no_fallback = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!no_fallback.passed);
    assert_eq!(no_fallback.mode, "container");
    assert_eq!(no_fallback.mode_requested, "container");
    assert_eq!(no_fallback.mode_used, "container");
    assert_eq!(no_fallback.fallback_reason, None);
    assert!(no_fallback.results.is_empty());
    assert!(
        no_fallback
            .error
            .as_deref()
            .is_some_and(|message| message.contains("container runtime unavailable"))
    );

    let force_fallback = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: Some(AetherVerifyMode::Container),
            fallback_to_host_on_unavailable: Some(true),
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(force_fallback.passed);
    assert_eq!(force_fallback.mode, "host");
    assert_eq!(force_fallback.mode_requested, "container");
    assert_eq!(force_fallback.mode_used, "host");
    assert_eq!(force_fallback.result_count, 1);
    assert_eq!(force_fallback.results[0].command, "cargo --version");
    assert_eq!(force_fallback.results[0].exit_code, Some(0));
    assert!(
        force_fallback
            .fallback_reason
            .as_deref()
            .is_some_and(|message| message.contains("container runtime unavailable"))
    );

    Ok(())
}

#[cfg(feature = "verification")]
#[test]
fn mcp_verify_handles_unavailable_microvm_runtime_with_optional_fallback() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();

    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(workspace.join("vmlinux"), "")?;
    fs::write(workspace.join("rootfs.ext4"), "")?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[verify]
mode = "microvm"
commands = ["cargo --version"]

[verify.container]
runtime = "definitely-missing-container-runtime"
image = "rust:1-bookworm"
workdir = "/workspace"
fallback_to_host_on_unavailable = false

[verify.microvm]
runtime = "definitely-missing-microvm-runtime"
kernel_image = "./vmlinux"
rootfs_image = "./rootfs.ext4"
workdir = "/workspace"
vcpu_count = 1
memory_mib = 1024
fallback_to_container_on_unavailable = false
fallback_to_host_on_unavailable = false
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let no_fallback = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: None,
            fallback_to_host_on_unavailable: None,
            fallback_to_container_on_unavailable: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!no_fallback.passed);
    assert_eq!(no_fallback.mode, "microvm");
    assert_eq!(no_fallback.mode_requested, "microvm");
    assert_eq!(no_fallback.mode_used, "microvm");
    assert_eq!(no_fallback.fallback_reason, None);
    assert!(no_fallback.results.is_empty());
    assert!(
        no_fallback
            .error
            .as_deref()
            .is_some_and(|message| message.contains("microvm runtime unavailable"))
    );

    let fallback_chain = rt
        .block_on(server.aether_verify(Parameters(AetherVerifyRequest {
            commands: None,
            mode: Some(AetherVerifyMode::Microvm),
            fallback_to_host_on_unavailable: Some(true),
            fallback_to_container_on_unavailable: Some(true),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(fallback_chain.passed);
    assert_eq!(fallback_chain.mode, "host");
    assert_eq!(fallback_chain.mode_requested, "microvm");
    assert_eq!(fallback_chain.mode_used, "host");
    assert_eq!(fallback_chain.result_count, 1);
    assert_eq!(fallback_chain.results[0].command, "cargo --version");
    assert_eq!(fallback_chain.results[0].exit_code, Some(0));
    assert!(
        fallback_chain
            .fallback_reason
            .as_deref()
            .is_some_and(|message| message.contains("microvm runtime unavailable"))
    );

    Ok(())
}
