//! Immutable evidence retained when a declaration leaves the active index.
//!
//! An ID can be reintroduced, so each retirement has its own identity. Historical
//! observations are never silently restored into a newly active declaration.

use std::collections::BTreeMap;

use rusqlite::types::ValueRef;
use serde_json::Value;

use super::*;

pub type ArchivedEvidence = BTreeMap<String, Vec<BTreeMap<String, Value>>>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolRetirementRecord {
    pub retirement_id: i64,
    pub symbol_id: String,
    pub retired_at: i64,
    pub reason: String,
    pub source_hash: Option<String>,
    pub symbol: Option<SymbolRecord>,
    /// Lossless SQLite rows, with column names retained. Blobs and non-finite
    /// reals use explicitly tagged objects rather than lossy JSON conversions.
    pub evidence: ArchivedEvidence,
}

/// Bounded discovery metadata. Searching retirements never reads evidence_json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRetirementSummary {
    pub retirement_id: i64,
    pub symbol_id: String,
    pub retired_at: i64,
    pub reason: String,
    pub symbol: SymbolRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolLineageRecord {
    pub retirement_id: i64,
    pub old_symbol_id: String,
    pub new_symbol_id: String,
    pub relation: String,
    pub confidence: String,
    pub evidence: String,
    pub source_revision: Option<String>,
    pub destination_revision: Option<String>,
    pub recorded_at: i64,
}

/// Historical lookup is explicit: active SIR version selectors continue to refer
/// to one active incarnation, while this interface retains every retired one.
pub trait SymbolHistoryStore {
    fn list_symbol_retirements(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<SymbolRetirementRecord>, StoreError>;
    fn list_symbol_lineage(&self, symbol_id: &str) -> Result<Vec<SymbolLineageRecord>, StoreError>;
    /// Search at most 16 literal terms in retired names/paths, returning at most
    /// 100 small metadata rows. Archived evidence is read only by explicit ID.
    fn search_symbol_retirement_summaries(
        &self,
        terms: &[String],
        limit: u32,
    ) -> Result<Vec<SymbolRetirementSummary>, StoreError>;
}

fn archived_value(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => value.into(),
        ValueRef::Real(value) if value.is_finite() => value.into(),
        ValueRef::Real(value) => serde_json::json!({"$sqlite_real": value.to_string()}),
        ValueRef::Text(value) => match std::str::from_utf8(value) {
            Ok(value) => Value::String(value.to_owned()),
            Err(_) => serde_json::json!({"$sqlite_text_bytes": value}),
        },
        ValueRef::Blob(value) => serde_json::json!({"$sqlite_blob": value}),
    }
}

fn snapshot_rows(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    symbol_id: &str,
) -> Result<Vec<BTreeMap<String, Value>>, StoreError> {
    // Table and column names come solely from the static list below.
    let mut statement = tx.prepare(&format!(
        "SELECT * FROM {table} WHERE {column} = ?1 ORDER BY rowid"
    ))?;
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let rows = statement.query_map([symbol_id], |row| {
        columns
            .iter()
            .enumerate()
            .map(|(index, column)| Ok((column.clone(), archived_value(row.get_ref(index)?))))
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub(crate) fn archive_symbol_tx(
    tx: &Transaction<'_>,
    symbol_id: &str,
    reason: &str,
) -> Result<Option<i64>, StoreError> {
    let mut evidence = ArchivedEvidence::new();
    for (table, column) in [
        ("symbols", "id"),
        ("sir", "id"),
        ("sir_history", "symbol_id"),
        ("sir_fingerprint_history", "symbol_id"),
        ("intent_contracts", "symbol_id"),
        ("intent_violations", "symbol_id"),
        ("test_intents", "symbol_id"),
        ("drift_results", "symbol_id"),
        ("sir_quality", "sir_id"),
        ("sir_audit", "symbol_id"),
        ("symbol_edges", "source_id"),
        ("community_snapshot", "symbol_id"),
    ] {
        evidence.insert(
            table.to_owned(),
            snapshot_rows(tx, table, column, symbol_id)?,
        );
    }
    // Audit rows are normally cascade-deleted with the SIR. A legacy orphan can
    // survive without its parent; that retained row alone is not a new retirement.
    if evidence
        .iter()
        .filter(|(table, _)| table.as_str() != "sir_audit")
        .all(|(_, rows)| rows.is_empty())
    {
        return Ok(None);
    }
    let symbol = evidence["symbols"]
        .first()
        .map(|row| {
            serde_json::from_value::<SymbolRecord>(Value::Object(row.clone().into_iter().collect()))
        })
        .transpose()?;
    let source_hash = evidence["sir"]
        .first()
        .and_then(|row| row.get("source_hash"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    tx.execute(
        "INSERT INTO symbol_retirements (symbol_id, retired_at, reason, source_hash, symbol_json, evidence_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![symbol_id, time::current_unix_timestamp_secs(), reason, source_hash,
            symbol.as_ref().map(serde_json::to_string).transpose()?, serde_json::to_string(&evidence)?],
    )?;
    Ok(Some(tx.last_insert_rowid()))
}

pub(crate) fn record_signature_lineage_tx(
    tx: &Transaction<'_>,
    retirement_id: i64,
    old_id: &str,
    new_id: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO symbol_lineage (retirement_id, old_symbol_id, new_symbol_id, relation, confidence, evidence, recorded_at) VALUES (?1, ?2, ?3, 'signature_change_candidate', 'heuristic', ?4, ?5)",
        params![retirement_id, old_id, new_id,
            "Unique old/new declarations share file, language, kind and qualified name; their signatures differ. This is a candidate, not proof of semantic continuity.",
            time::current_unix_timestamp_secs()],
    )?;
    Ok(())
}

impl SymbolHistoryStore for SqliteStore {
    fn search_symbol_retirement_summaries(
        &self,
        terms: &[String],
        limit: u32,
    ) -> Result<Vec<SymbolRetirementSummary>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut patterns = Vec::new();
        for term in terms {
            let term = term.trim().to_lowercase();
            if term.is_empty() {
                continue;
            }
            let pattern = format!(
                "%{}%",
                term.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            if !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
            if patterns.len() == 16 {
                break;
            }
        }
        if patterns.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while searching retirement metadata".to_owned(),
            )
        })?;
        if !schema::table_exists(&connection, "symbol_retirements")? {
            return Ok(Vec::new());
        }
        let matches = (1..=patterns.len()).map(|index| format!(
            "CASE WHEN LOWER(json_extract(symbol_json, '$.qualified_name')) LIKE ?{index} ESCAPE '\\' OR LOWER(json_extract(symbol_json, '$.file_path')) LIKE ?{index} ESCAPE '\\' THEN 1 ELSE 0 END"
        )).collect::<Vec<_>>().join(" + ");
        let sql = format!(
            "SELECT retirement_id, symbol_id, retired_at, reason, symbol_json FROM
             (SELECT retirement_id, symbol_id, retired_at, reason, symbol_json,
             ({matches}) AS matched_terms FROM symbol_retirements WHERE symbol_json IS NOT NULL)
             WHERE matched_terms > 0
             ORDER BY matched_terms DESC, retired_at DESC, retirement_id DESC LIMIT ?{}",
            patterns.len() + 1
        );
        let mut parameters = patterns.into_iter().map(SqlValue::Text).collect::<Vec<_>>();
        parameters.push(SqlValue::Integer(i64::from(limit.min(100))));
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(parameters), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (retirement_id, symbol_id, retired_at, reason, symbol_json) = row?;
            Ok(SymbolRetirementSummary {
                retirement_id,
                symbol_id,
                retired_at,
                reason,
                symbol: serde_json::from_str(&symbol_json)?,
            })
        })
        .collect()
    }

    fn list_symbol_retirements(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<SymbolRetirementRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while reading retirement history".to_owned(),
            )
        })?;
        // Read-only schema-21 stores remain useful before a writable upgrade.
        if !schema::table_exists(&conn, "symbol_retirements")? {
            return Ok(Vec::new());
        }
        let mut statement = conn.prepare("SELECT retirement_id, symbol_id, retired_at, reason, source_hash, symbol_json, evidence_json FROM symbol_retirements WHERE symbol_id = ?1 ORDER BY retirement_id")?;
        let rows = statement.query_map([symbol_id.trim()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (
                retirement_id,
                symbol_id,
                retired_at,
                reason,
                source_hash,
                symbol_json,
                evidence_json,
            ) = row?;
            Ok(SymbolRetirementRecord {
                retirement_id,
                symbol_id,
                retired_at,
                reason,
                source_hash,
                symbol: symbol_json
                    .map(|json| serde_json::from_str(&json))
                    .transpose()?,
                evidence: serde_json::from_str(&evidence_json)?,
            })
        })
        .collect()
    }

    fn list_symbol_lineage(&self, symbol_id: &str) -> Result<Vec<SymbolLineageRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while reading lineage history".to_owned(),
            )
        })?;
        if !schema::table_exists(&conn, "symbol_lineage")? {
            return Ok(Vec::new());
        }
        let mut statement = conn.prepare("SELECT retirement_id, old_symbol_id, new_symbol_id, relation, confidence, evidence, source_revision, destination_revision, recorded_at FROM symbol_lineage WHERE old_symbol_id = ?1 OR new_symbol_id = ?1 ORDER BY retirement_id")?;
        let rows = statement.query_map([symbol_id.trim()], |row| {
            Ok(SymbolLineageRecord {
                retirement_id: row.get(0)?,
                old_symbol_id: row.get(1)?,
                new_symbol_id: row.get(2)?,
                relation: row.get(3)?,
                confidence: row.get(4)?,
                evidence: row.get(5)?,
                source_revision: row.get(6)?,
                destination_revision: row.get(7)?,
                recorded_at: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}
