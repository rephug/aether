//! Memory notes, blast radius, test intents and the audit tools.

use super::*;

#[test]
fn mcp_memory_tools_dedup_and_recall_fallback_work() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let first = rt
        .block_on(server.aether_remember(Parameters(AetherRememberRequest {
            content: "We selected sqlite for deterministic local persistence.".to_owned(),
            tags: Some(vec!["architecture".to_owned()]),
            entity_refs: None,
            file_refs: Some(vec!["crates/aether-store/src/lib.rs".to_owned()]),
            symbol_refs: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(first.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(first.action, "created");
    assert_eq!(first.tags, vec!["architecture".to_owned()]);

    let second = rt
        .block_on(server.aether_remember(Parameters(AetherRememberRequest {
            content: "We selected sqlite for deterministic local persistence.".to_owned(),
            tags: Some(vec!["database".to_owned()]),
            entity_refs: None,
            file_refs: None,
            symbol_refs: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(second.note_id, first.note_id);
    assert_eq!(second.action, "updated_existing");
    assert_eq!(
        second.tags,
        vec!["architecture".to_owned(), "database".to_owned()]
    );

    let recall = rt
        .block_on(server.aether_recall(Parameters(AetherRecallRequest {
            query: "why sqlite".to_owned(),
            mode: Some(SearchMode::Semantic),
            limit: Some(5),
            include_archived: Some(false),
            tags_filter: Some(vec!["architecture".to_owned()]),
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(recall.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(recall.mode_requested, SearchMode::Semantic);
    assert_eq!(recall.mode_used, SearchMode::Lexical);
    assert_eq!(
        recall.fallback_reason.as_deref(),
        Some(SEARCH_FALLBACK_EMBEDDINGS_DISABLED)
    );
    assert_eq!(recall.result_count, 1);
    assert_eq!(recall.notes[0].note_id, first.note_id);
    assert_eq!(
        recall.notes[0].tags,
        vec!["architecture".to_owned(), "database".to_owned()]
    );

    let session_note = rt
        .block_on(
            server.aether_session_note(Parameters(AetherRememberRequest {
                content: "Refactoring payment flow to reduce batch memory usage.".to_owned(),
                tags: Some(vec!["session".to_owned(), "refactor".to_owned()]),
                entity_refs: None,
                file_refs: Some(vec!["src/payments/processor.rs".to_owned()]),
                symbol_refs: Some(vec!["sym-payment".to_owned()]),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(session_note.schema_version, MEMORY_SCHEMA_VERSION);
    assert_eq!(session_note.action, "created");
    assert_eq!(session_note.source_type, "session");

    let store = SqliteStore::open(workspace)?;
    let stored_session_note = store
        .get_project_note(session_note.note_id.as_str())?
        .expect("session note should be persisted");
    assert_eq!(stored_session_note.source_type, "session");

    Ok(())
}

#[test]
fn mcp_memory_tool_response_schema_shapes_are_stable() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);
    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let remember = rt
        .block_on(server.aether_remember(Parameters(AetherRememberRequest {
            content: "Design rationale note".to_owned(),
            tags: Some(vec!["design".to_owned()]),
            entity_refs: None,
            file_refs: None,
            symbol_refs: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let remember_json = serde_json::to_value(&remember)?;
    let remember_obj = remember_json
        .as_object()
        .expect("remember response should serialize as object");
    for key in [
        "schema_version",
        "note_id",
        "action",
        "content_hash",
        "tags",
        "created_at",
    ] {
        assert!(remember_obj.contains_key(key), "missing key: {key}");
    }

    let session_note = rt
        .block_on(
            server.aether_session_note(Parameters(AetherRememberRequest {
                content: "Session note content".to_owned(),
                tags: Some(vec!["session".to_owned()]),
                entity_refs: None,
                file_refs: None,
                symbol_refs: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let session_note_json = serde_json::to_value(&session_note)?;
    let session_note_obj = session_note_json
        .as_object()
        .expect("session note response should serialize as object");
    for key in ["schema_version", "note_id", "action", "source_type"] {
        assert!(session_note_obj.contains_key(key), "missing key: {key}");
    }

    let recall = rt
        .block_on(server.aether_recall(Parameters(AetherRecallRequest {
            query: "design".to_owned(),
            mode: Some(SearchMode::Lexical),
            limit: Some(5),
            include_archived: Some(false),
            tags_filter: None,
        })))
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    let recall_json = serde_json::to_value(&recall)?;
    let recall_obj = recall_json
        .as_object()
        .expect("recall response should serialize as object");
    for key in [
        "schema_version",
        "query",
        "mode_requested",
        "mode_used",
        "fallback_reason",
        "result_count",
        "notes",
    ] {
        assert!(recall_obj.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[test]
fn mcp_blast_radius_response_schema_shape_is_stable() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "sqlite"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;
    let response = rt
        .block_on(
            server.aether_blast_radius(Parameters(AetherBlastRadiusRequest {
                file: "src/lib.rs".to_owned(),
                min_risk: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;

    let json = serde_json::to_value(&response)?;
    let object = json
        .as_object()
        .expect("blast radius response should serialize as object");
    for key in [
        "schema_version",
        "target_file",
        "mining_state",
        "coupled_files",
        "test_guards",
    ] {
        assert!(object.contains_key(key), "missing key: {key}");
    }

    Ok(())
}

#[test]
fn mcp_test_intents_tool_and_blast_radius_return_test_guards() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    fs::create_dir_all(workspace.join(".aether"))?;
    fs::write(
        workspace.join(".aether/config.toml"),
        r#"[storage]
graph_backend = "cozo"

[embeddings]
enabled = false
provider = "qwen3_local"
vector_backend = "sqlite"
"#,
    )?;
    fs::create_dir_all(workspace.join("src"))?;
    fs::create_dir_all(workspace.join("tests"))?;
    fs::write(workspace.join("src/payment.rs"), "fn charge() {}\n")?;
    fs::write(
        workspace.join("tests/payment_test.rs"),
        "#[test]\nfn test_charge() {}\n",
    )?;

    let store = SqliteStore::open(workspace)?;
    store.replace_test_intents_for_file(
        "tests/payment_test.rs",
        &[
            TestIntentRecord {
                intent_id: "intent-1".to_owned(),
                file_path: "tests/payment_test.rs".to_owned(),
                test_name: "test_charge".to_owned(),
                intent_text: "charges correctly".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: None,
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
            },
            TestIntentRecord {
                intent_id: "intent-2".to_owned(),
                file_path: "tests/payment_test.rs".to_owned(),
                test_name: "test_errors".to_owned(),
                intent_text: "handles invalid input".to_owned(),
                group_label: None,
                language: "rust".to_owned(),
                symbol_id: None,
                created_at: 1_700_000_000_000,
                updated_at: 1_700_000_000_000,
            },
        ],
    )?;

    drop(store);

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let intents = rt
        .block_on(
            server.aether_test_intents(Parameters(AetherTestIntentsRequest {
                file: Some("tests/payment_test.rs".to_owned()),
                symbol_id: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(intents.result_count, 2);
    assert!(
        intents
            .intents
            .iter()
            .any(|entry| entry.intent_text == "charges correctly")
    );

    let blast = rt
        .block_on(
            server.aether_blast_radius(Parameters(AetherBlastRadiusRequest {
                file: "src/payment.rs".to_owned(),
                min_risk: None,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(!blast.test_guards.is_empty());
    assert_eq!(blast.test_guards[0].test_file, "tests/payment_test.rs");
    assert!(
        blast.test_guards[0]
            .intents
            .contains(&"charges correctly".to_owned())
    );
    assert_eq!(blast.test_guards[0].inference_method, "naming_convention");

    Ok(())
}

#[test]
fn mcp_audit_tools_submit_report_and_resolve_findings() -> Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path();
    write_test_config(workspace);

    let store = SqliteStore::open(workspace)?;
    store.upsert_symbol(custom_symbol_record(
        "sym-audit",
        "crate::audit::handle",
        "crates/aether-store/src/lib.rs",
        "function",
    ))?;
    store.write_sir_blob(
        "sym-audit",
        r#"{
            "intent":"seed audit sir",
            "inputs":[],
            "outputs":[],
            "side_effects":[],
            "dependencies":[],
            "error_modes":[],
            "confidence":0.8
        }"#,
    )?;
    drop(store);

    let server = AetherMcpServer::new(workspace, false)?;
    let rt = Runtime::new()?;

    let submit = rt
        .block_on(
            server.aether_audit_submit(Parameters(AetherAuditSubmitRequest {
                symbol_id: "sym-audit".to_owned(),
                audit_type: None,
                severity: AuditSeverity::High,
                category: AuditCategory::SilentFailure,
                certainty: AuditCertainty::Confirmed,
                trigger_condition: "rollback path skipped".to_owned(),
                impact: "partial writes remain committed".to_owned(),
                description: "reconcile skips rollback on partial failure".to_owned(),
                related_symbols: Some(vec!["sym-helper".to_owned()]),
                model: None,
                provider: None,
                reasoning: Some("reproduced with targeted fixture".to_owned()),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(submit.finding_id > 0);
    assert_eq!(submit.status, AuditStatus::Open);

    let report = rt
        .block_on(
            server.aether_audit_report(Parameters(AetherAuditReportRequest {
                crate_filter: Some("aether-store".to_owned()),
                min_severity: Some(AuditSeverity::Low),
                category: None,
                status: None,
                limit: Some(25),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(report.summary.total, 1);
    assert_eq!(report.summary.high, 1);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(
        report.findings[0].qualified_name.as_deref(),
        Some("crate::audit::handle")
    );
    assert_eq!(
        report.findings[0].file_path.as_deref(),
        Some("crates/aether-store/src/lib.rs")
    );

    let resolve = rt
        .block_on(
            server.aether_audit_resolve(Parameters(AetherAuditResolveRequest {
                finding_id: submit.finding_id,
                status: AuditStatus::Fixed,
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert!(resolve.resolved);
    assert_eq!(resolve.new_status, AuditStatus::Fixed);

    let fixed_report = rt
        .block_on(
            server.aether_audit_report(Parameters(AetherAuditReportRequest {
                crate_filter: Some("aether-store".to_owned()),
                min_severity: Some(AuditSeverity::Low),
                category: None,
                status: Some(AuditStatus::Fixed),
                limit: Some(25),
            })),
        )
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .0;
    assert_eq!(fixed_report.summary.total, 1);
    assert_eq!(fixed_report.findings.len(), 1);
    assert_eq!(fixed_report.findings[0].status, AuditStatus::Fixed);

    Ok(())
}
