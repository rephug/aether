use super::*;

fn new_audit_finding(
    symbol_id: &str,
    severity: &str,
    category: &str,
    status: &str,
) -> NewAuditFinding {
    NewAuditFinding {
        symbol_id: symbol_id.to_owned(),
        audit_type: "symbol".to_owned(),
        severity: severity.to_owned(),
        category: category.to_owned(),
        certainty: "confirmed".to_owned(),
        trigger_condition: format!("trigger for {symbol_id}"),
        impact: format!("impact for {symbol_id}"),
        description: format!("description for {symbol_id}"),
        related_symbols: Vec::new(),
        model: "claude_code".to_owned(),
        provider: "manual".to_owned(),
        reasoning: Some(format!("reasoning for {symbol_id}")),
        status: status.to_owned(),
    }
}

fn seed_symbol_with_sir(store: &SqliteStore, record: SymbolRecord) {
    let symbol_id = record.id.clone();
    let sir_hash = format!("hash-{}", symbol_id);
    let sir_json = format!(
        "{{\"intent\":\"Handle {symbol_id}\",\"side_effects\":[],\"error_modes\":[],\"confidence\":0.4}}"
    );
    store.upsert_symbol(record).expect("upsert symbol");
    upsert_sir_state(store, &symbol_id, &sir_hash, &sir_json, 1_700_000_500);
}

#[test]
fn insert_and_query_audit_findings_round_trip() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_symbol_with_sir(&store, symbol_record());

    let finding_id = store
        .insert_audit_finding(NewAuditFinding {
            related_symbols: vec!["sym-2".to_owned()],
            ..new_audit_finding("sym-1", "high", "logic_error", "open")
        })
        .expect("insert audit finding");

    let findings = store
        .query_audit_findings(&AuditFindingFilters {
            symbol_id: Some("sym-1".to_owned()),
            ..AuditFindingFilters::default()
        })
        .expect("query audit findings");

    assert_eq!(findings.len(), 1);
    let finding = &findings[0];
    assert_eq!(finding.id, finding_id);
    assert_eq!(finding.symbol_id, "sym-1");
    assert_eq!(finding.audit_type, "symbol");
    assert_eq!(finding.severity, "high");
    assert_eq!(finding.category, "logic_error");
    assert_eq!(finding.certainty, "confirmed");
    assert_eq!(finding.related_symbols, vec!["sym-2".to_owned()]);
    assert_eq!(finding.status, "open");
    assert!(finding.created_at > 0);
    assert_eq!(finding.resolved_at, None);
}

#[test]
fn audit_findings_filter_by_min_severity() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_symbol_with_sir(&store, symbol_record());

    for severity in ["informational", "low", "medium", "critical"] {
        store
            .insert_audit_finding(new_audit_finding(
                "sym-1",
                severity,
                "silent_failure",
                "open",
            ))
            .expect("insert audit finding");
    }

    let findings = store
        .query_audit_findings(&AuditFindingFilters {
            min_severity: Some("medium".to_owned()),
            ..AuditFindingFilters::default()
        })
        .expect("query findings with severity filter");

    let severities = findings
        .iter()
        .map(|finding| finding.severity.as_str())
        .collect::<Vec<_>>();
    assert_eq!(severities, vec!["critical", "medium"]);
}

#[test]
fn resolve_audit_finding_updates_status_and_timestamp() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");
    seed_symbol_with_sir(&store, symbol_record());

    let finding_id = store
        .insert_audit_finding(new_audit_finding("sym-1", "medium", "state", "open"))
        .expect("insert audit finding");

    let resolved = store
        .resolve_audit_finding(finding_id, "fixed")
        .expect("resolve audit finding");
    assert!(resolved);

    let findings = store
        .query_audit_findings(&AuditFindingFilters {
            status: Some("fixed".to_owned()),
            ..AuditFindingFilters::default()
        })
        .expect("query resolved finding");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, finding_id);
    assert_eq!(findings[0].status, "fixed");
    assert!(findings[0].resolved_at.is_some());
}

#[test]
fn count_audit_findings_by_severity_respects_scope_filters() {
    let temp = tempdir().expect("tempdir");
    let store = SqliteStore::open(temp.path()).expect("open store");

    let mut store_symbol = symbol_record();
    store_symbol.file_path = "crates/aether-store/src/lib.rs".to_owned();
    seed_symbol_with_sir(&store, store_symbol);

    let mut mcp_symbol = symbol_record_ts();
    mcp_symbol.id = "sym-3".to_owned();
    mcp_symbol.qualified_name = "mcp::handle".to_owned();
    mcp_symbol.file_path = "crates/aether-mcp/src/lib.rs".to_owned();
    seed_symbol_with_sir(&store, mcp_symbol);

    store
        .insert_audit_finding(new_audit_finding(
            "sym-1",
            "critical",
            "logic_error",
            "open",
        ))
        .expect("insert critical finding");
    store
        .insert_audit_finding(new_audit_finding("sym-1", "high", "silent_failure", "open"))
        .expect("insert high finding");
    store
        .insert_audit_finding(new_audit_finding(
            "sym-3",
            "informational",
            "encoding",
            "open",
        ))
        .expect("insert informational finding");

    let counts = store
        .count_audit_findings_by_severity(&AuditFindingFilters {
            file_path_prefix: Some("crates/aether-store".to_owned()),
            min_severity: Some("low".to_owned()),
            status: Some("open".to_owned()),
            ..AuditFindingFilters::default()
        })
        .expect("count findings by severity");

    assert_eq!(counts.total, 2);
    assert_eq!(counts.critical, 1);
    assert_eq!(counts.high, 1);
    assert_eq!(counts.medium, 0);
    assert_eq!(counts.low, 0);
    assert_eq!(counts.informational, 0);
}
