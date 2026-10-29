//! Regression: a signature retirement must not make unresolved audit evidence invisible.
use super::*;
use aether_store::AuditStore;

#[test]
fn mcp_audit_retirement_is_disclosed_after_restore_without_reassignment() -> Result<()> {
    retirement_case(true)
}

#[test]
fn mcp_pruned_audit_is_disclosed_after_restore_without_reassignment() -> Result<()> {
    retirement_case(false)
}

fn retirement_case(migrate: bool) -> Result<()> {
    let temp = tempdir()?;
    write_test_config(temp.path());
    let store = SqliteStore::open(temp.path())?;
    let old = SymbolRecord {
        id: "audit-old".into(),
        file_path: "crates/aether-store/src/fixture.rs".into(),
        language: "rust".into(),
        kind: "function".into(),
        qualified_name: "fixture::audited".into(),
        signature_fingerprint: "old-signature".into(),
        last_seen_at: 1,
    };
    store.upsert_symbol(old.clone())?;
    seed_sir_with_pass(&store, &old.id, "scan")?;
    let rt = Runtime::new()?;
    let server = AetherMcpServer::new(temp.path(), false)?;
    let finding = rt
        .block_on(
            server.aether_audit_submit(Parameters(AetherAuditSubmitRequest {
                symbol_id: old.id.clone(),
                audit_type: None,
                severity: AuditSeverity::High,
                category: AuditCategory::LogicError,
                certainty: AuditCertainty::Confirmed,
                trigger_condition: "synthetic retirement".into(),
                impact: "unresolved finding disappears from active report".into(),
                description: "retain original finding attribution".into(),
                related_symbols: None,
                model: None,
                provider: None,
                reasoning: None,
            })),
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .0;
    let new = SymbolRecord {
        id: "audit-new".into(),
        signature_fingerprint: "new-signature".into(),
        ..old.clone()
    };
    if migrate {
        store.publish_and_reconcile_symbols(
            std::slice::from_ref(&new),
            &[(old.id.clone(), new.id.clone())],
            &[],
        )?;
    } else {
        store.publish_and_reconcile_symbols(&[], &[], std::slice::from_ref(&old.id))?;
    }
    let lineage = aether_store::SymbolHistoryStore::list_symbol_lineage(&store, &old.id)?;
    assert_eq!(lineage.len(), usize::from(migrate));
    assert!(
        store
            .query_audit_findings(&aether_store::AuditFindingFilters::default())?
            .is_empty()
    );
    assert!(!store.resolve_audit_finding(finding.finding_id, "fixed")?);
    drop(server);
    drop(store);

    // All fixture writers are closed. Copy a checkpointed synthetic DB into a
    // second workspace so the assertion covers reopen/restore, not an in-memory cache.
    let db = Connection::open(temp.path().join(".aether/meta.sqlite"))?;
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(db);
    let restored = tempdir()?;
    fs::create_dir_all(restored.path().join(".aether"))?;
    fs::copy(
        temp.path().join(".aether/meta.sqlite"),
        restored.path().join(".aether/meta.sqlite"),
    )?;
    fs::copy(
        temp.path().join(".aether/config.toml"),
        restored.path().join(".aether/config.toml"),
    )?;
    let server = AetherMcpServer::new(restored.path(), false)?;
    let report = rt
        .block_on(
            server.aether_audit_report(Parameters(AetherAuditReportRequest {
                // Historical disclosure is explicitly global, independent of these active filters.
                crate_filter: Some("another-crate".into()),
                min_severity: Some(AuditSeverity::Critical),
                category: None,
                status: Some(AuditStatus::Fixed),
                limit: Some(1),
            })),
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .0;
    assert!(report.findings.is_empty());
    assert_eq!(report.summary.total, 0);
    let json = serde_json::to_value(&report)?;
    assert_eq!(json["historical"]["unresolved_observations"], 1);
    assert_eq!(
        json["historical"]["scope"],
        "retained_retirement_archives_unfiltered"
    );
    assert_eq!(json["historical"]["scan_complete"], true);
    assert_eq!(json["historical"]["findings"][0]["symbol_id"], old.id);
    assert_eq!(
        json["historical"]["findings"][0]["finding_id"],
        finding.finding_id
    );
    assert_eq!(
        json["historical"]["findings"][0]["status_at_retirement"],
        "open"
    );
    let store = SqliteStore::open(restored.path())?;
    assert!(
        store
            .query_audit_findings(&aether_store::AuditFindingFilters {
                symbol_id: Some(new.id),
                ..Default::default()
            })?
            .is_empty()
    );
    assert!(!store.resolve_audit_finding(finding.finding_id, "fixed")?);
    // Reintroducing the original ID is a new active incarnation, not permission
    // to rehydrate or reassign an archived audit.
    store.upsert_symbol(old.clone())?;
    seed_sir_with_pass(&store, &old.id, "scan")?;
    assert!(
        store
            .query_audit_findings(&aether_store::AuditFindingFilters {
                symbol_id: Some(old.id),
                ..Default::default()
            })?
            .is_empty()
    );
    Ok(())
}

#[test]
fn mcp_legacy_audit_report_missing_history_deserializes_as_unknown() -> Result<()> {
    let report: aether_mcp::AetherAuditReportResponse =
        serde_json::from_value(serde_json::json!({
            "findings": [],
            "summary": {"total":0,"critical":0,"high":0,"medium":0,"low":0,"informational":0}
        }))?;
    assert_eq!(report.historical.scope, "not_reported");
    assert!(!report.historical.scan_complete);
    assert!(!report.historical.retention_available);
    Ok(())
}
