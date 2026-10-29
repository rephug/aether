//! Bounded disclosure of unresolved observations retained at symbol retirement.
//! These rows are history, not findings reassigned to a heuristic successor.
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::StoreError;

const MAX_RETIREMENTS: usize = 128;
const MAX_ARCHIVE_BYTES: usize = 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_FINDINGS: usize = 50;
const DESCRIPTION_CHARS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetiredAuditFinding {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetiredAuditDisclosure {
    pub retention_available: bool,
    /// Complete archive scan, not proof of current code safety or resolution.
    pub scan_complete: bool,
    pub retirement_scan_limit: u32,
    pub retirements_considered: u32,
    pub retirements_skipped: u32,
    pub more_retirements: bool,
    /// Observations can repeat across retirements; this is not a distinct-ID count.
    pub unresolved_observations: u32,
    /// Additional observations in scanned archives, beyond the returned samples.
    pub samples_omitted: u32,
    pub findings: Vec<RetiredAuditFinding>,
}

impl Default for RetiredAuditDisclosure {
    fn default() -> Self {
        Self {
            retention_available: false,
            scan_complete: false,
            retirement_scan_limit: MAX_RETIREMENTS as u32,
            retirements_considered: 0,
            retirements_skipped: 0,
            more_retirements: false,
            unresolved_observations: 0,
            samples_omitted: 0,
            findings: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct AuditEnvelope {
    // Missing/invalid audit coverage is unknown, never an empty success.
    sir_audit: Vec<ArchivedAudit>,
}

#[derive(Deserialize)]
struct ArchivedAudit {
    id: i64,
    symbol_id: String,
    status: String,
    severity: String,
    category: String,
    description: String,
}

pub(crate) fn read_retired_audits(
    conn: &Connection,
    sample_limit: u32,
) -> Result<RetiredAuditDisclosure, StoreError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
    let mut disclosure = RetiredAuditDisclosure::default();
    let available = tx
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='symbol_retirements'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !available {
        return Ok(disclosure);
    }
    disclosure.retention_available = true;
    // octet_length on a column reads byte-length metadata without materializing
    // oversized bodies (supported by the bundled SQLite 3.46). Budgets count
    // stored-encoding bytes; UTF-16 to UTF-8 decoding can expand them.
    let mut metadata = {
        let mut stmt = tx.prepare(
            "SELECT retirement_id, symbol_id, retired_at, octet_length(evidence_json)
             FROM symbol_retirements ORDER BY retirement_id DESC LIMIT ?1",
        )?;
        stmt.query_map([MAX_RETIREMENTS as i64 + 1], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, usize>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    disclosure.more_retirements = metadata.len() > MAX_RETIREMENTS;
    metadata.truncate(MAX_RETIREMENTS);
    let sample_limit = (sample_limit as usize).clamp(1, MAX_FINDINGS);
    let mut bytes_read = 0usize;
    for (retirement_id, symbol_id, retired_at, bytes) in metadata {
        disclosure.retirements_considered += 1;
        if bytes > MAX_ARCHIVE_BYTES || bytes > MAX_TOTAL_BYTES.saturating_sub(bytes_read) {
            disclosure.retirements_skipped += 1;
            continue;
        }
        let raw: String = tx.query_row(
            "SELECT evidence_json FROM symbol_retirements WHERE retirement_id=?1",
            [retirement_id],
            |row| row.get(0),
        )?;
        bytes_read += bytes;
        let envelope = match serde_json::from_str::<AuditEnvelope>(&raw) {
            Ok(envelope) => envelope,
            Err(_) => {
                disclosure.retirements_skipped += 1;
                continue;
            }
        };
        // Validate a whole retirement before counting it. Unknown status or
        // attribution means incomplete coverage, even if other rows look valid.
        if envelope.sir_audit.iter().any(|audit| {
            audit.id <= 0
                || audit.severity.chars().count() > 32
                || audit.category.chars().count() > 64
                || audit.symbol_id != symbol_id
                || !matches!(
                    audit.status.trim().to_ascii_lowercase().as_str(),
                    "open" | "confirmed" | "fixed" | "wontfix"
                )
        }) {
            disclosure.retirements_skipped += 1;
            continue;
        }
        for audit in envelope.sir_audit {
            let status = audit.status.trim().to_ascii_lowercase();
            if !matches!(status.as_str(), "open" | "confirmed") {
                continue;
            }
            disclosure.unresolved_observations += 1;
            if disclosure.findings.len() >= sample_limit {
                continue;
            }
            let description_preview: String =
                audit.description.chars().take(DESCRIPTION_CHARS).collect();
            let description_truncated = description_preview.len() < audit.description.len();
            disclosure.findings.push(RetiredAuditFinding {
                retirement_id,
                symbol_id: symbol_id.clone(),
                retired_at,
                finding_id: audit.id,
                status_at_retirement: status,
                severity: audit.severity,
                category: audit.category,
                description_preview,
                description_truncated,
            });
        }
    }
    disclosure.samples_omitted =
        disclosure.unresolved_observations - disclosure.findings.len() as u32;
    disclosure.scan_complete = !disclosure.more_retirements && disclosure.retirements_skipped == 0;
    tx.commit()?;
    Ok(disclosure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn database() -> Connection {
        let db = Connection::open_in_memory().expect("fixture");
        db.execute_batch(
            "CREATE TABLE symbol_retirements (
               retirement_id INTEGER PRIMARY KEY, symbol_id TEXT NOT NULL,
               retired_at INTEGER NOT NULL, evidence_json TEXT NOT NULL);",
        )
        .expect("schema");
        db
    }

    fn audit(id: i64, status: &str) -> serde_json::Value {
        json!({"id":id,"symbol_id":"old","status":status,"severity":"high",
               "category":"logic_error","description":"original finding"})
    }

    fn insert(db: &Connection, id: i64, evidence: &str) {
        db.execute(
            "INSERT INTO symbol_retirements VALUES(?1,'old',1,?2)",
            rusqlite::params![id, evidence],
        )
        .expect("archive");
    }

    #[test]
    fn legacy_retention_is_unknown_and_empty_retention_is_complete() {
        let db = Connection::open_in_memory().expect("fixture");
        let result = read_retired_audits(&db, 10).expect("legacy disclosure");
        assert!(!result.retention_available);
        assert!(!result.scan_complete);
        let result = read_retired_audits(&database(), 10).expect("empty disclosure");
        assert!(result.retention_available);
        assert!(result.scan_complete);
    }

    #[test]
    fn unresolved_observations_keep_original_ids_and_status() {
        let db = database();
        insert(
            &db,
            7,
            &json!({"sir_audit":[audit(1,"open"),audit(2,"confirmed"),
            audit(3,"fixed"),audit(4,"wontfix")]})
            .to_string(),
        );
        db.execute_batch("PRAGMA query_only=ON;")
            .expect("read-only fixture");
        let changes = db.total_changes();
        let result = read_retired_audits(&db, 10).expect("disclosure");
        assert!(result.scan_complete);
        assert_eq!(result.unresolved_observations, 2);
        assert_eq!(result.findings[0].retirement_id, 7);
        assert_eq!(result.findings[0].symbol_id, "old");
        assert_eq!(result.findings[1].finding_id, 2);
        assert_eq!(result.findings[1].status_at_retirement, "confirmed");
        assert_eq!(db.total_changes(), changes);
    }

    #[test]
    fn malformed_or_missing_coverage_never_reports_complete() {
        for raw in [
            "{}",
            "not json",
            r#"{"sir_audit":null}"#,
            r#"{"sir_audit":[{"id":1,"symbol_id":"other","status":"open","severity":"high","category":"state","description":"x"}]}"#,
            r#"{"sir_audit":[{"id":1,"symbol_id":"old","status":"mystery","severity":"high","category":"state","description":"x"}]}"#,
        ] {
            let db = database();
            insert(&db, 1, raw);
            insert(&db, 2, &json!({"sir_audit":[audit(2,"open")]}).to_string());
            let result = read_retired_audits(&db, 10).expect("disclosure");
            assert!(!result.scan_complete, "{raw}");
            assert_eq!(result.retirements_skipped, 1);
            assert_eq!(result.unresolved_observations, 1);
        }
    }

    #[test]
    fn scan_and_body_bounds_disclose_unknown_history() {
        let db = database();
        insert(&db, 1, &" ".repeat(MAX_ARCHIVE_BYTES + 1));
        let result = read_retired_audits(&db, 1).expect("oversized");
        assert!(!result.scan_complete);
        assert_eq!(result.retirements_skipped, 1);
        for id in 2..=MAX_RETIREMENTS as i64 + 2 {
            insert(&db, id, r#"{"sir_audit":[]}"#);
        }
        let result = read_retired_audits(&db, 1).expect("bounded scan");
        assert_eq!(result.retirements_considered, MAX_RETIREMENTS as u32);
        assert!(result.more_retirements);
        assert!(!result.scan_complete);
    }

    #[test]
    fn aggregate_bytes_and_returned_samples_are_bounded() {
        let db = database();
        let raw = json!({"sir_audit":[audit(1,"open")],"other":"x".repeat(900_000)}).to_string();
        for id in 1..=10 {
            insert(&db, id, &raw);
        }
        let result = read_retired_audits(&db, 1).expect("bounded disclosure");
        assert!(!result.scan_complete);
        assert!(result.retirements_skipped > 0);
        assert_eq!(result.findings.len(), 1);
        assert_eq!(result.samples_omitted, result.unresolved_observations - 1);
    }

    #[test]
    fn unicode_previews_are_bounded_without_changing_archive() {
        let db = database();
        let mut finding = audit(1, "open");
        finding["description"] = json!("界".repeat(DESCRIPTION_CHARS + 1));
        let raw = json!({"sir_audit":[finding]}).to_string();
        insert(&db, 1, &raw);
        let result = read_retired_audits(&db, 10).expect("disclosure");
        assert!(result.findings[0].description_truncated);
        assert_eq!(
            result.findings[0].description_preview.chars().count(),
            DESCRIPTION_CHARS
        );
        let stored: String = db
            .query_row("SELECT evidence_json FROM symbol_retirements", [], |r| {
                r.get(0)
            })
            .expect("unchanged archive");
        assert_eq!(stored, raw);
    }
    #[test]
    fn oversized_metadata_is_unknown_and_sample_limit_is_clamped() {
        for field in ["severity", "category"] {
            let db = database();
            let mut finding = audit(1, "open");
            finding[field] = json!("x".repeat(65));
            insert(&db, 1, &json!({"sir_audit":[finding]}).to_string());
            let result = read_retired_audits(&db, 10).expect("metadata bound");
            assert!(!result.scan_complete);
            assert_eq!(result.retirements_skipped, 1);
        }
        let db = database();
        let findings: Vec<_> = (1..=100).map(|id| audit(id, "open")).collect();
        insert(&db, 1, &json!({"sir_audit":findings}).to_string());
        let result = read_retired_audits(&db, u32::MAX).expect("maximum samples");
        assert!(result.scan_complete);
        assert_eq!(result.findings.len(), MAX_FINDINGS);
        assert_eq!(result.samples_omitted, 50);
    }

    #[test]
    fn repeated_original_id_is_two_attributed_observations_not_a_new_finding() {
        let db = database();
        let raw = json!({"sir_audit":[audit(1,"open")]}).to_string();
        insert(&db, 1, &raw);
        insert(&db, 2, &raw);
        let result = read_retired_audits(&db, 10).expect("repeated observation");
        assert_eq!(result.unresolved_observations, 2);
        assert_eq!(result.findings[0].finding_id, result.findings[1].finding_id);
        assert_ne!(
            result.findings[0].retirement_id,
            result.findings[1].retirement_id
        );
    }
}
