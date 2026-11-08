use std::process::Command;

use aether_store::{
    AuditStore, NewAuditFinding, SirStateStore, SqliteStore, SymbolCatalogStore, SymbolRecord,
};
use tempfile::tempdir;

fn seed_observation(store: &SqliteStore, id: &str, file_path: &str, severity: &str) {
    store
        .upsert_symbol(SymbolRecord {
            id: id.to_owned(),
            file_path: file_path.to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: "fixture::apply_payment".to_owned(),
            signature_fingerprint: "fixture".to_owned(),
            last_seen_at: 1_700_000_000,
        })
        .expect("seed symbol");
    store
        .write_sir_blob(id, r#"{"intent":"Manually seeded fixture explanation"}"#)
        .expect("seed explanation");
    store
        .insert_audit_finding(NewAuditFinding {
            symbol_id: id.to_owned(),
            audit_type: "symbol".to_owned(),
            severity: severity.to_owned(),
            category: "arithmetic".to_owned(),
            certainty: "confirmed".to_owned(),
            trigger_condition: "fixture overflow boundary".to_owned(),
            impact: "fixture payment cannot be applied".to_owned(),
            description: format!("manually seeded observation for {file_path}"),
            related_symbols: Vec::new(),
            model: "manual_fixture".to_owned(),
            provider: "manual_fixture".to_owned(),
            reasoning: None,
            status: "open".to_owned(),
        })
        .expect("seed finding");
}

#[test]
fn scoped_audit_report_does_not_let_a_neighbor_hide_the_requested_change() {
    let workspace = tempdir().expect("workspace");
    let store = SqliteStore::open(workspace.path()).expect("fixture catalog");
    seed_observation(&store, "target", "crates/pay_core/src/lib.rs", "high");
    seed_observation(&store, "neighbor", "crates/payxcore/src/lib.rs", "critical");
    drop(store);

    for scope in ["pay_core", "crates/pay_core/src/lib.rs", "crates/pay%/src/"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aetherd"));
        command
            .arg("--workspace")
            .arg(workspace.path())
            .args(["audit-report", "--crate", scope, "--limit", "1"])
            .env_clear();
        for name in ["PATH", "LD_LIBRARY_PATH"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let output = command.output().expect("run report");
        assert!(output.status.success(), "{output:?}");
        let report = String::from_utf8(output.stdout).expect("UTF-8 report");
        if scope.contains('%') {
            assert!(
                report.contains("Showing 0 of 0 matching findings."),
                "{report}"
            );
        } else {
            assert!(
                report.contains("Showing 1 of 1 matching findings."),
                "{report}"
            );
            assert!(report.contains("Finding ID: 1\n"), "{report}");
            assert!(
                report.contains("File: crates/pay_core/src/lib.rs\n"),
                "{report}"
            );
            assert!(report.contains("Summary: 0 critical, 1 high"), "{report}");
        }
        assert!(!report.contains("payxcore"), "{report}");
        assert!(!report.contains("Report truncated"), "{report}");
    }
}
