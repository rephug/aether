//! `aether_sir_inject` behaviour, its embedding refresh and `aether_audit_candidates`.

use super::*;

#[test]
fn mcp_sir_inject_tool_injects_blocks_and_forces_overwrites() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    let store = SqliteStore::open(workspace)?;
    store.upsert_symbol(custom_symbol_record(
        "sym-inject",
        "crate::inject::target",
        "src/lib.rs",
        "function",
    ))?;
    drop(store);

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let first = rt
        .block_on(server.aether_sir_inject(Parameters(AetherSirInjectRequest {
            symbol: "sym-inject".to_owned(),
            intent: "Initial injected intent".to_owned(),
            behavior: Some("Loads state and writes cache".to_owned()),
            edge_cases: Some("Cold cache triggers a full reload".to_owned()),
            side_effects: Some(vec!["writes cache".to_owned()]),
            dependencies: Some(vec!["SqliteStore".to_owned()]),
            error_modes: Some(vec!["io".to_owned()]),
            confidence: Some(0.9),
            inputs: Some(vec!["symbol_id".to_owned()]),
            outputs: Some(vec!["Result<(), io::Error>".to_owned()]),
            complexity: Some("Medium".to_owned()),
            generation_pass: None,
            model: None,
            provider: None,
            force: None,
            source_hash: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(first.status, "injected");
    assert_eq!(first.sir_version, 1);
    assert_eq!(first.embedding_status, "skipped: embeddings disabled");
    assert!(
        first
            .note
            .as_deref()
            .is_some_and(|note| note.contains("embeddings disabled"))
    );

    let blocked = rt
        .block_on(server.aether_sir_inject(Parameters(AetherSirInjectRequest {
            symbol: "crate::inject::target".to_owned(),
            intent: "Blocked overwrite".to_owned(),
            behavior: None,
            edge_cases: None,
            side_effects: None,
            dependencies: None,
            error_modes: None,
            confidence: Some(0.4),
            inputs: None,
            outputs: None,
            complexity: None,
            generation_pass: None,
            model: None,
            provider: None,
            force: Some(false),
            source_hash: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(blocked.status, "blocked");
    assert_eq!(blocked.previous_confidence, Some(0.9));

    let forced = rt
        .block_on(server.aether_sir_inject(Parameters(AetherSirInjectRequest {
            symbol: "sym-inject".to_owned(),
            intent: "Forced overwrite".to_owned(),
            behavior: Some("Updates cache and metadata".to_owned()),
            edge_cases: Some("Network failures leave stale cache entries".to_owned()),
            side_effects: Some(vec!["updates cache".to_owned()]),
            dependencies: Some(vec!["SqliteStore".to_owned()]),
            error_modes: Some(vec!["network".to_owned()]),
            confidence: Some(0.4),
            inputs: Some(vec!["symbol_id".to_owned()]),
            outputs: Some(vec!["AetherSirInjectResponse".to_owned()]),
            complexity: Some("High".to_owned()),
            generation_pass: Some("deep".to_owned()),
            model: Some("claude-opus-4-6".to_owned()),
            provider: Some("manual".to_owned()),
            force: Some(true),
            source_hash: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(forced.status, "injected");
    assert_eq!(forced.previous_confidence, Some(0.9));
    assert!(forced.sir_version >= 2);

    let store = SqliteStore::open(workspace)?;
    let meta = store
        .get_sir_meta("sym-inject")?
        .expect("injected sir meta exists");
    assert_eq!(meta.sir_hash, forced.sir_hash);
    assert_eq!(meta.sir_version, forced.sir_version);

    Ok(())
}

#[test]
fn mcp_sir_inject_refreshes_embedding_when_embeddings_are_enabled() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    let (endpoint, _server_thread) = spawn_stub_embedding_server();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        format!(
            r#"[inference]
provider = "qwen3_local"
api_key_env = "GEMINI_API_KEY"

[storage]
mirror_sir_files = true
graph_backend = "sqlite"

[embeddings]
enabled = true
provider = "qwen3_local"
vector_backend = "sqlite"
endpoint = "{endpoint}"
model = "stub-embed"
"#
        ),
    )?;

    let store = SqliteStore::open(workspace)?;
    store.upsert_symbol(custom_symbol_record(
        "sym-embed",
        "crate::inject::embed",
        "src/lib.rs",
        "function",
    ))?;
    drop(store);

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let injected = rt
        .block_on(server.aether_sir_inject(Parameters(AetherSirInjectRequest {
            symbol: "sym-embed".to_owned(),
            intent: "Injected intent that must reach the vector store".to_owned(),
            behavior: None,
            edge_cases: None,
            side_effects: None,
            dependencies: None,
            error_modes: None,
            confidence: Some(0.97),
            inputs: None,
            outputs: None,
            complexity: None,
            generation_pass: None,
            model: None,
            provider: None,
            force: None,
            source_hash: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(injected.status, "injected");
    assert_eq!(injected.embedding_status, "refreshed");
    assert_eq!(injected.note, None);

    // The vector store row now carries the injected SIR's hash.
    let pipeline =
        aetherd::sir_pipeline::SirPipeline::new_embeddings_only(workspace.to_path_buf())?;
    let record = pipeline
        .load_symbol_embedding("sym-embed")?
        .expect("embedding row written by inject");
    assert_eq!(record.sir_hash, injected.sir_hash);
    // The provider L2-normalizes what the endpoint returned; the direction is the stub's.
    assert_eq!(record.embedding.len(), 4);
    let norm = record
        .embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "embedding should be unit length: {norm}"
    );
    let expected = [0.1f32, 0.2, 0.3, 0.4];
    let expected_norm = expected.iter().map(|v| v * v).sum::<f32>().sqrt();
    for (actual, raw) in record.embedding.iter().zip(expected) {
        assert!(
            (actual - raw / expected_norm).abs() < 1e-4,
            "{actual} vs {raw}"
        );
    }

    // A second inject with the same content leaves the row alone.
    let again = rt
        .block_on(server.aether_sir_inject(Parameters(AetherSirInjectRequest {
            symbol: "sym-embed".to_owned(),
            intent: "Injected intent that must reach the vector store".to_owned(),
            behavior: None,
            edge_cases: None,
            side_effects: None,
            dependencies: None,
            error_modes: None,
            confidence: Some(0.97),
            inputs: None,
            outputs: None,
            complexity: None,
            generation_pass: None,
            model: None,
            provider: None,
            force: Some(true),
            source_hash: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(again.embedding_status, "unchanged");

    Ok(())
}

#[test]
fn mcp_audit_candidates_excludes_deep_sirs_unless_requested() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    let store = SqliteStore::open(workspace)?;
    store.upsert_symbol(custom_symbol_record(
        "sym-scan",
        "crate::audit::scan_level",
        "src/scan.rs",
        "function",
    ))?;
    store.upsert_symbol(custom_symbol_record(
        "sym-deep",
        "crate::audit::deep_level",
        "src/deep.rs",
        "function",
    ))?;
    seed_sir_with_pass(&store, "sym-scan", "scan")?;
    seed_sir_with_pass(&store, "sym-deep", "deep")?;
    // A deep attempt that failed: pass label says deep, but there is no SIR at all.
    store.upsert_symbol(custom_symbol_record(
        "sym-deep-failed",
        "crate::audit::deep_failed",
        "src/failed.rs",
        "function",
    ))?;
    store.upsert_sir_meta(SirMetaRecord {
        id: "sym-deep-failed".to_owned(),
        sir_hash: String::new(),
        sir_version: 1,
        provider: "seed".to_owned(),
        model: "seed".to_owned(),
        generation_pass: "deep".to_owned(),
        reasoning_trace: None,
        prompt_hash: None,
        staleness_score: None,
        updated_at: 0,
        sir_status: "stale".to_owned(),
        last_error: Some("provider timed out".to_owned()),
        last_attempt_at: 1_700_000_100,
    })?;
    drop(store);

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let default_response = rt
        .block_on(
            server.aether_audit_candidates(Parameters(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: None,
                file_filter: None,
                min_risk: None,
                include_reasoning_hints: None,
                include_deep: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let mut default_ids = default_response
        .candidates
        .iter()
        .map(|candidate| candidate.symbol_id.as_str())
        .collect::<Vec<_>>();
    default_ids.sort_unstable();
    assert_eq!(default_ids, vec!["sym-deep-failed", "sym-scan"]);
    assert_eq!(default_response.total_in_scope, 2);

    let with_deep = rt
        .block_on(
            server.aether_audit_candidates(Parameters(AetherAuditCandidatesRequest {
                top_n: Some(10),
                crate_filter: None,
                file_filter: None,
                min_risk: None,
                include_reasoning_hints: None,
                include_deep: Some(true),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let mut with_deep_ids = with_deep
        .candidates
        .iter()
        .map(|candidate| candidate.symbol_id.as_str())
        .collect::<Vec<_>>();
    with_deep_ids.sort_unstable();
    assert_eq!(
        with_deep_ids,
        vec!["sym-deep", "sym-deep-failed", "sym-scan"]
    );
    assert_eq!(with_deep.total_in_scope, 3);

    Ok(())
}
