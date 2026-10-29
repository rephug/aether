//! Explicit historical audit disclosure; active filters do not hide this history.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoricalAuditFinding {
    pub retirement_id: i64,
    pub symbol_id: String,
    pub retired_at: i64,
    pub finding_id: i64,
    pub status_at_retirement: String,
    pub severity: String,
    pub category: String,
    pub description_preview: String,
    pub description_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoricalAuditDisclosure {
    pub scope: String,
    pub notice: String,
    pub retention_available: bool,
    pub scan_complete: bool,
    pub retirement_scan_limit: u32,
    pub retirements_considered: u32,
    pub retirements_skipped: u32,
    pub more_retirements: bool,
    pub unresolved_observations: u32,
    pub samples_omitted: u32,
    pub findings: Vec<HistoricalAuditFinding>,
}

impl From<aether_store::RetiredAuditDisclosure> for HistoricalAuditDisclosure {
    fn from(value: aether_store::RetiredAuditDisclosure) -> Self {
        Self {
            scope: "retained_retirement_archives_unfiltered".to_owned(),
            notice: "Active findings/summary exclude retired evidence. Historical observations were open or confirmed at retirement; they are not reassigned to a successor and are not resolved by the active finding API. Counts cover scanned archives, may repeat an original finding across retirements, and do not establish current code safety. Scans retained archives only; pre-retention losses and unarchived legacy findings are not covered. Incomplete coverage is not an all-clear. Inspect aether_symbol_timeline for the original symbol and full retirement evidence.".to_owned(),
            retention_available: value.retention_available,
            scan_complete: value.scan_complete,
            retirement_scan_limit: value.retirement_scan_limit,
            retirements_considered: value.retirements_considered,
            retirements_skipped: value.retirements_skipped,
            more_retirements: value.more_retirements,
            unresolved_observations: value.unresolved_observations,
            samples_omitted: value.samples_omitted,
            findings: value.findings.into_iter().map(|finding| HistoricalAuditFinding {
                retirement_id: finding.retirement_id,
                symbol_id: finding.symbol_id,
                retired_at: finding.retired_at,
                finding_id: finding.finding_id,
                status_at_retirement: finding.status_at_retirement,
                severity: finding.severity,
                category: finding.category,
                description_preview: finding.description_preview,
                description_truncated: finding.description_truncated,
            }).collect(),
        }
    }
}

impl Default for HistoricalAuditDisclosure {
    fn default() -> Self {
        let mut value = Self::from(aether_store::RetiredAuditDisclosure::default());
        value.scope = "not_reported".to_owned();
        value.notice = "Historical disclosure was omitted by this server response; coverage is unknown. No complete-history or safety inference can be made.".to_owned();
        value
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn omitted_disclosure_is_unknown_rather_than_complete_empty_history() {
        let value = HistoricalAuditDisclosure::default();
        assert_eq!(value.scope, "not_reported");
        assert!(!value.retention_available);
        assert!(!value.scan_complete);
        assert!(value.notice.contains("omitted"));
    }
}
