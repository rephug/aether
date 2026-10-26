//! Tool profiles over the MCP protocol: what `tools/list` shows and what `tools/call`
//! accepts for the full and compact profiles.

use super::*;

use aether_config::{COMPACT_MCP_TOOLS, McpToolProfile};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ErrorCode, Tool};
use rmcp::service::{RoleClient, RunningService};

/// Every tool the full profile serves without the `verification` feature. A new tool
/// belongs here, and is worth weighing for `COMPACT_MCP_TOOLS`.
const FULL_TOOLS: [&str; 40] = [
    "aether_acknowledge_drift",
    "aether_ask",
    "aether_audit_candidates",
    "aether_audit_cross_symbol",
    "aether_audit_report",
    "aether_audit_resolve",
    "aether_audit_submit",
    "aether_blast_radius",
    "aether_call_chain",
    "aether_contract_add",
    "aether_contract_check",
    "aether_contract_dismiss",
    "aether_contract_list",
    "aether_contract_remove",
    "aether_contract_violations",
    "aether_dependencies",
    "aether_drift_report",
    "aether_enhance_prompt",
    "aether_explain",
    "aether_get_sir",
    "aether_health",
    "aether_health_explain",
    "aether_health_hotspots",
    "aether_preflight",
    "aether_recall",
    "aether_refactor_prep",
    "aether_remember",
    "aether_search",
    "aether_session_note",
    "aether_sir_context",
    "aether_sir_inject",
    "aether_status",
    "aether_suggest_trait_split",
    "aether_symbol_lookup",
    "aether_symbol_timeline",
    "aether_test_intents",
    "aether_trace_cause",
    "aether_usage_matrix",
    "aether_verify_intent",
    "aether_why_changed",
];

/// Serve `server` over an in-memory pipe and connect a client to it.
async fn connect(server: AetherMcpServer) -> Result<RunningService<RoleClient, ()>> {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    Ok(().serve(client_io).await?)
}

fn tool_names(tools: &[Tool]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.to_string()).collect()
}

fn call(name: &str, arguments: serde_json::Value) -> CallToolRequestParams {
    CallToolRequestParams {
        meta: None,
        name: name.to_owned().into(),
        arguments: arguments.as_object().cloned(),
        task: None,
    }
}

fn server_with_profile(workspace: &Path, profile: McpToolProfile) -> Result<AetherMcpServer> {
    Ok(AetherMcpServer::new(workspace, false)?.with_tool_profile(profile))
}

#[test]
fn compact_profile_lists_exactly_the_compact_tools() -> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let server = server_with_profile(temp.path(), McpToolProfile::Compact)?;

    Runtime::new()?.block_on(async {
        let client = connect(server).await?;
        let tools = client.list_all_tools().await?;
        client.cancel().await?;

        let mut expected = COMPACT_MCP_TOOLS.map(str::to_owned).to_vec();
        expected.sort();
        assert_eq!(tool_names(&tools), expected, "listed in name order");
        for tool in &tools {
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                !description.is_empty() && description.len() <= 260,
                "{} needs a short description, got {} bytes",
                tool.name,
                description.len()
            );
            assert!(
                tool.output_schema.is_none(),
                "{} lists an output schema",
                tool.name
            );
            assert_eq!(
                tool.input_schema.get("type"),
                Some(&serde_json::json!("object"))
            );
            assert!(!tool.input_schema.contains_key("$schema"));
        }
        let lookup = tools
            .iter()
            .find(|tool| tool.name == "aether_symbol_lookup")
            .expect("lookup listed");
        let lookup_properties = lookup.input_schema["properties"]
            .as_object()
            .expect("lookup properties");
        assert!(
            !lookup_properties.contains_key("symbol_ids")
                && !lookup_properties.contains_key("include_source"),
            "the /scan-only lookup parameters stay out of the compact schema"
        );
        Ok(())
    })
}

#[test]
fn full_profile_is_unchanged_and_the_default() -> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let server = AetherMcpServer::new(temp.path(), false)?;
    assert_eq!(server.tool_profile(), McpToolProfile::Full);

    Runtime::new()?.block_on(async {
        let client = connect(server).await?;
        let tools = client.list_all_tools().await?;
        client.cancel().await?;

        #[cfg(not(feature = "verification"))]
        assert_eq!(tool_names(&tools), FULL_TOOLS.map(str::to_owned).to_vec());
        #[cfg(feature = "verification")]
        {
            let mut expected = FULL_TOOLS.map(str::to_owned).to_vec();
            expected.push("aether_verify".to_owned());
            expected.sort();
            assert_eq!(tool_names(&tools), expected);
        }

        // The compact profile's definitions don't leak into the full one.
        let lookup = tools
            .iter()
            .find(|tool| tool.name == "aether_symbol_lookup")
            .expect("lookup listed");
        assert!(
            lookup
                .description
                .as_deref()
                .unwrap_or_default()
                .starts_with(
                    "Lookup symbols by qualified name or file path, or fetch exact symbol_ids"
                )
        );
        assert!(
            lookup.input_schema["properties"]
                .get("symbol_ids")
                .is_some()
        );
        assert!(tools.iter().all(|tool| tool.output_schema.is_some()));
        Ok(())
    })
}

#[test]
fn tool_outside_the_compact_profile_returns_a_clear_error() -> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let server = server_with_profile(temp.path(), McpToolProfile::Compact)?;

    Runtime::new()?.block_on(async {
        let client = connect(server).await?;
        let refused = client
            .call_tool(call("aether_health", serde_json::json!({})))
            .await
            .expect_err("aether_health is outside the compact profile");
        let unknown = client
            .call_tool(call("aether_no_such_tool", serde_json::json!({})))
            .await
            .expect_err("unknown tool");
        client.cancel().await?;

        let rmcp::ServiceError::McpError(refused) = refused else {
            panic!("expected an MCP error, got {refused:?}");
        };
        assert_eq!(refused.code, ErrorCode::INVALID_PARAMS);
        for expected in [
            "aether_health is not available",
            "compact tool profile",
            "aether_dependencies",
            "--tools full",
            "[mcp]",
        ] {
            assert!(
                refused.message.contains(expected),
                "missing {expected:?} in {:?}",
                refused.message
            );
        }

        // A name no profile knows keeps rmcp's own error.
        let rmcp::ServiceError::McpError(unknown) = unknown else {
            panic!("expected an MCP error, got {unknown:?}");
        };
        assert!(!unknown.message.contains("profile"), "{}", unknown.message);
        Ok(())
    })
}

#[test]
fn compact_tools_accept_their_advertised_arguments() -> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let server = server_with_profile(temp.path(), McpToolProfile::Compact)?;

    Runtime::new()?.block_on(async {
        let client = connect(server).await?;
        let dependencies = client
            .call_tool(call(
                "aether_dependencies",
                serde_json::json!({"symbol_id": "missing"}),
            ))
            .await?;
        let search = client
            .call_tool(call(
                "aether_search",
                serde_json::json!({"query": "alpha", "mode": "hybrid", "limit": 5}),
            ))
            .await?;
        let context = client
            .call_tool(call(
                "aether_sir_context",
                serde_json::json!({
                    "symbol": "missing",
                    "max_tokens": 2000,
                    "include_layers": ["source", "sir", "graph", "tests", "health", "reasoning"],
                }),
            ))
            .await;
        client.cancel().await?;

        let dependencies = dependencies.structured_content.expect("structured result");
        assert_eq!(dependencies["found"], false);
        let search = search.structured_content.expect("structured result");
        assert_eq!(search["mode_requested"], "hybrid");
        assert_eq!(search["limit"], 5);
        // The selector is resolved after every other argument parsed, layers included.
        let Err(rmcp::ServiceError::McpError(context)) = context else {
            panic!("expected the unknown selector to fail, got {context:?}");
        };
        assert_eq!(context.message, "symbol not found: missing");
        Ok(())
    })
}

#[test]
fn workspace_config_selects_the_profile_and_with_tool_profile_overrides_it() -> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let config_path = temp.path().join(".aether/config.toml");
    let config = fs::read_to_string(&config_path)?;
    fs::write(
        &config_path,
        format!("{config}\n[mcp]\ntools = \"compact\"\n"),
    )?;

    let server = AetherMcpServer::new(temp.path(), false)?;
    assert_eq!(server.tool_profile(), McpToolProfile::Compact);
    let server = server.with_tool_profile(McpToolProfile::Full);
    assert_eq!(server.tool_profile(), McpToolProfile::Full);
    Ok(())
}
