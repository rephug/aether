use std::collections::HashMap;

use aether_store::{AuditFinding, AuditSeverityCounts, SymbolSearchResult};
use chrono::{DateTime, Utc};

fn display_symbol_name(qualified_name: Option<&str>, symbol_id: &str) -> String {
    qualified_name
        .and_then(|value| {
            value
                .rsplit("::")
                .next()
                .filter(|segment| !segment.trim().is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| symbol_id.to_owned())
}

fn format_found_date(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

pub fn render_audit_report(
    scope_label: &str,
    status: &str,
    min_severity: &str,
    findings: &[AuditFinding],
    symbol_records: &HashMap<String, SymbolSearchResult>,
    summary: &AuditSeverityCounts,
) -> String {
    let mut output = String::from("AETHER Audit Report\n===================\n");
    output.push_str(
        format!("Scope: {scope_label} | Status: {status} | Min severity: {min_severity}\n\n")
            .as_str(),
    );

    if findings.is_empty() {
        output.push_str("No audit findings matched the requested filters.\n\n");
    } else {
        for finding in findings {
            let symbol = symbol_records.get(&finding.symbol_id);
            let symbol_name = display_symbol_name(
                symbol.map(|record| record.qualified_name.as_str()),
                &finding.symbol_id,
            );
            let file_path = symbol
                .map(|record| record.file_path.as_str())
                .unwrap_or("<unknown>");

            output.push_str(
                format!(
                    "[{}] {} - {}\n",
                    finding.severity.to_ascii_uppercase(),
                    finding.category,
                    symbol_name
                )
                .as_str(),
            );
            output.push_str(format!("  File: {file_path}\n").as_str());
            output.push_str(format!("  Description: {}\n", finding.description).as_str());
            output.push_str(format!("  Certainty: {}\n", finding.certainty).as_str());
            output.push_str(format!("  Status: {}\n", finding.status).as_str());
            output.push_str(
                format!("  Found: {}\n\n", format_found_date(finding.created_at)).as_str(),
            );
        }
    }

    output.push_str(
        format!(
            "Summary: {} critical, {} high, {} medium, {} low",
            summary.critical, summary.high, summary.medium, summary.low
        )
        .as_str(),
    );
    if summary.informational > 0 {
        output.push_str(format!(", {} informational", summary.informational).as_str());
    }
    output.push_str(format!(" ({} total)\n", summary.total).as_str());
    output
}

#[cfg(test)]
mod tests {
    use super::render_audit_report;
    use aether_store::{AuditFinding, AuditSeverityCounts, SymbolSearchResult};

    #[test]
    fn render_audit_report_formats_findings_and_summary() {
        let findings = vec![AuditFinding {
            id: 1,
            symbol_id: "sym-reconcile".to_owned(),
            audit_type: "symbol".to_owned(),
            severity: "high".to_owned(),
            category: "silent_failure".to_owned(),
            certainty: "confirmed".to_owned(),
            trigger_condition: "partial transaction failure".to_owned(),
            impact: "orphaned rows".to_owned(),
            description: "sir_quality rows orphaned on reconciliation".to_owned(),
            related_symbols: Vec::new(),
            model: "claude_code".to_owned(),
            provider: "manual".to_owned(),
            reasoning: None,
            status: "open".to_owned(),
            created_at: 1_772_310_400,
            resolved_at: None,
        }];
        let symbols = [(
            "sym-reconcile".to_owned(),
            SymbolSearchResult {
                symbol_id: "sym-reconcile".to_owned(),
                qualified_name: "crate::lifecycle::reconcile_and_prune".to_owned(),
                file_path: "crates/aether-store/src/lifecycle.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                access_count: 0,
                last_accessed_at: None,
            },
        )]
        .into_iter()
        .collect();
        let summary = AuditSeverityCounts {
            total: 12,
            critical: 0,
            high: 3,
            medium: 7,
            low: 2,
            informational: 0,
        };

        let rendered = render_audit_report(
            "aether-store",
            "open",
            "low",
            findings.as_slice(),
            &symbols,
            &summary,
        );

        assert!(rendered.contains("AETHER Audit Report"));
        assert!(rendered.contains("Scope: aether-store | Status: open | Min severity: low"));
        assert!(rendered.contains("[HIGH] silent_failure - reconcile_and_prune"));
        assert!(rendered.contains("File: crates/aether-store/src/lifecycle.rs"));
        assert!(rendered.contains("Description: sir_quality rows orphaned on reconciliation"));
        assert!(rendered.contains("Certainty: confirmed"));
        assert!(rendered.contains("Status: open"));
        assert!(rendered.contains("Summary: 0 critical, 3 high, 7 medium, 2 low (12 total)"));
    }
}
