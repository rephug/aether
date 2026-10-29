//! Durable retirement work for projections outside the SQLite transaction.
//!
//! This queue records deletion work; it does not publish graph generations or
//! establish cross-store consistency. Mirrors have a separate durability contract.

use super::*;

const MAX_LIST_LIMIT: u32 = 1000;
const MAX_SUMMARY_SAMPLES: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementProjection {
    Graph,
    Vector,
}

impl RetirementProjection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Vector => "vector",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementCleanupStatus {
    Pending,
    Done,
    SupersededByReintroduction,
    LegacyUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementCleanupRecord {
    pub retirement_id: i64,
    pub projection: RetirementProjection,
    pub symbol_id: String,
    pub backend: String,
    pub status: RetirementCleanupStatus,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementCleanupSummary {
    /// False on a read-only store that predates this queue.
    pub available: bool,
    pub pending_graph: u64,
    pub pending_vector: u64,
    pub failed_components: u64,
    pub superseded_components: u64,
    pub legacy_unknown_components: u64,
    pub oldest_pending_at: Option<i64>,
    pub samples: Vec<RetirementCleanupRecord>,
    pub omitted_components: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetirementCleanupOutcome {
    Completed,
    AlreadyCompleted,
    SupersededByReintroduction,
    LegacyUnknown,
}

/// Backend identity is captured when the store is opened, before any retirement
/// transaction. Reopen after changing backend configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetirementBackends {
    graph: &'static str,
    vector: &'static str,
}

impl RetirementBackends {
    pub(crate) fn from_config(config: &aether_config::AetherConfig) -> Self {
        Self {
            graph: match config.storage.graph_backend {
                GraphBackend::Surreal | GraphBackend::Cozo => "surreal",
                GraphBackend::Sqlite => "sqlite",
            },
            vector: config.embeddings.vector_backend.as_str(),
        }
    }
}

pub trait RetirementCleanupStore {
    /// Actionable pending components, bounded to 1..=1000 rows. Legacy-unknown
    /// records are never selected for automatic replay. For batches of at least
    /// two, reserve ceil(limit/4) slots for oldest tried work and the rest for
    /// fresh work, filling unused capacity from the other class. This prevents
    /// persistent failures and continuous new arrivals starving the other class.
    /// Limit one has no dual-class quota guarantee; use a larger replay batch.
    fn list_pending_retirement_cleanup(
        &self,
        limit: u32,
    ) -> Result<Vec<RetirementCleanupRecord>, StoreError>;
    /// Apply projection eligibility before quota selection and LIMIT. This lets
    /// a vector-only worker make progress without touching skipped graph jobs.
    fn list_pending_retirement_cleanup_for_projection(
        &self,
        projection: RetirementProjection,
        limit: u32,
    ) -> Result<Vec<RetirementCleanupRecord>, StoreError>;
    /// Counts plus at most 100 non-completed component samples. Empty means no
    /// recorded retirement backlog, never that every projection is fresh.
    fn retirement_cleanup_summary(
        &self,
        sample_limit: u32,
    ) -> Result<RetirementCleanupSummary, StoreError>;
    /// Attempt one external projection deletion while fencing catalog publication.
    ///
    /// The callback runs under this store's connection mutex and a SQLite
    /// IMMEDIATE write transaction. It MUST NOT reenter SQLite, open a backend,
    /// acquire another kind of write lock, or perform unbounded work. Initialize
    /// backend handles and acquire the vector embedding lock before calling this.
    /// Only external Surreal/Lance deletion belongs in the callback; SQLite
    /// projections were removed by the retirement transaction itself.
    ///
    /// A callback failure is recorded durably and remains pending. A crash after
    /// the backend delete but before acknowledgement is retried idempotently.
    fn attempt_retirement_cleanup(
        &self,
        retirement_id: i64,
        projection: RetirementProjection,
        expected_backend: &str,
        cleanup: &mut dyn FnMut(&str) -> Result<(), StoreError>,
    ) -> Result<RetirementCleanupOutcome, StoreError>;
}

pub(crate) fn enqueue_retirement_cleanup_tx(
    tx: &Transaction<'_>,
    retirement_id: i64,
    symbol_id: &str,
    backends: RetirementBackends,
) -> Result<(), StoreError> {
    for (projection, backend) in [
        (RetirementProjection::Graph, backends.graph),
        (RetirementProjection::Vector, backends.vector),
    ] {
        // Both SQLite projections are deleted in the same transaction as this
        // insertion. A failed deletion rolls these acknowledgements back too.
        let status = if backend == "sqlite" {
            "done"
        } else {
            "pending"
        };
        tx.execute(
            "INSERT INTO retirement_cleanup (retirement_id, projection, symbol_id, backend, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![retirement_id, projection.as_str(), symbol_id, backend, status,
                time::current_unix_timestamp_secs()],
        )?;
    }
    Ok(())
}

fn invalid_enum(index: usize, value: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(StoreError::Compatibility(format!(
            "invalid retirement cleanup enum: {value}"
        ))),
    )
}

fn cleanup_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetirementCleanupRecord> {
    let projection: String = row.get(1)?;
    let projection = match projection.as_str() {
        "graph" => RetirementProjection::Graph,
        "vector" => RetirementProjection::Vector,
        _ => return Err(invalid_enum(1, projection)),
    };
    let status: String = row.get(4)?;
    let status = match status.as_str() {
        "pending" => RetirementCleanupStatus::Pending,
        "done" => RetirementCleanupStatus::Done,
        "superseded_by_reintroduction" => RetirementCleanupStatus::SupersededByReintroduction,
        "legacy_unknown" => RetirementCleanupStatus::LegacyUnknown,
        _ => return Err(invalid_enum(4, status)),
    };
    Ok(RetirementCleanupRecord {
        retirement_id: row.get(0)?,
        projection,
        symbol_id: row.get(2)?,
        backend: row.get(3)?,
        status,
        attempts: row.get(5)?,
        last_error: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const RECORD_COLUMNS: &str = "retirement_id, projection, symbol_id, backend, status, attempts, last_error, created_at, updated_at";

fn select_pending_cleanup(
    conn: &Connection,
    projection: Option<RetirementProjection>,
    limit: u32,
) -> Result<Vec<RetirementCleanupRecord>, StoreError> {
    if !schema::table_exists(conn, "retirement_cleanup")? {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, MAX_LIST_LIMIT);
    let projection = projection.map(RetirementProjection::as_str);
    if limit == 1 {
        let mut statement = conn.prepare(&format!(
            "SELECT {RECORD_COLUMNS} FROM retirement_cleanup WHERE status = 'pending' AND (?1 IS NULL OR projection = ?1) ORDER BY updated_at, attempts, retirement_id, projection LIMIT 1"
        ))?;
        return statement
            .query_map([projection], cleanup_record_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into);
    }
    let retry_quota = limit.div_ceil(4);
    // One statement gives all quota branches the same SQLite snapshot, so a
    // component cannot move between classes and appear twice during selection.
    let sql = format!(
        "WITH eligible AS (
            SELECT {RECORD_COLUMNS} FROM retirement_cleanup
            WHERE status = 'pending' AND (?1 IS NULL OR projection = ?1)
        ), retries AS (
            SELECT * FROM eligible WHERE attempts > 0
            ORDER BY updated_at, attempts, retirement_id, projection LIMIT ?2
        ), fresh AS (
            SELECT * FROM eligible WHERE attempts = 0
            ORDER BY updated_at, retirement_id, projection
            LIMIT (?3 - (SELECT COUNT(*) FROM retries))
        ), extra_retries AS (
            SELECT e.* FROM eligible e WHERE e.attempts > 0
            AND NOT EXISTS (SELECT 1 FROM retries r WHERE r.retirement_id = e.retirement_id AND r.projection = e.projection)
            ORDER BY e.updated_at, e.attempts, e.retirement_id, e.projection
            LIMIT (?3 - (SELECT COUNT(*) FROM retries) - (SELECT COUNT(*) FROM fresh))
        ), chosen AS (
            SELECT 0 AS bucket, * FROM retries
            UNION ALL SELECT 1, * FROM fresh
            UNION ALL SELECT 2, * FROM extra_retries
        ) SELECT {RECORD_COLUMNS} FROM chosen
          ORDER BY bucket, updated_at, attempts, retirement_id, projection"
    );
    let mut statement = conn.prepare(&sql)?;
    statement
        .query_map(
            params![projection, retry_quota, limit],
            cleanup_record_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

impl RetirementCleanupStore for SqliteStore {
    fn list_pending_retirement_cleanup(
        &self,
        limit: u32,
    ) -> Result<Vec<RetirementCleanupRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while reading retirement cleanup".to_owned(),
            )
        })?;
        select_pending_cleanup(&conn, None, limit)
    }

    fn list_pending_retirement_cleanup_for_projection(
        &self,
        projection: RetirementProjection,
        limit: u32,
    ) -> Result<Vec<RetirementCleanupRecord>, StoreError> {
        let conn = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while reading retirement cleanup".to_owned(),
            )
        })?;
        select_pending_cleanup(&conn, Some(projection), limit)
    }

    fn retirement_cleanup_summary(
        &self,
        sample_limit: u32,
    ) -> Result<RetirementCleanupSummary, StoreError> {
        let conn = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while reading retirement cleanup summary".to_owned(),
            )
        })?;
        let mut summary = RetirementCleanupSummary {
            available: schema::table_exists(&conn, "retirement_cleanup")?,
            pending_graph: 0,
            pending_vector: 0,
            failed_components: 0,
            superseded_components: 0,
            legacy_unknown_components: 0,
            oldest_pending_at: None,
            samples: Vec::new(),
            omitted_components: 0,
        };
        if !summary.available {
            return Ok(summary);
        }
        let mut statement = conn.prepare("SELECT projection, status, COUNT(*), SUM(last_error IS NOT NULL), MIN(created_at) FROM retirement_cleanup WHERE status <> 'done' GROUP BY projection, status")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut total = 0;
        for row in rows {
            let (projection, status, count, errors, oldest) = row?;
            total += count;
            match status.as_str() {
                "pending" => {
                    if projection == "graph" {
                        summary.pending_graph += count;
                    } else {
                        summary.pending_vector += count;
                    }
                    summary.failed_components += errors;
                    summary.oldest_pending_at = Some(
                        summary
                            .oldest_pending_at
                            .map_or(oldest, |prior| prior.min(oldest)),
                    );
                }
                "superseded_by_reintroduction" => summary.superseded_components += count,
                "legacy_unknown" => summary.legacy_unknown_components += count,
                _ => {
                    return Err(StoreError::Compatibility(format!(
                        "unknown cleanup status: {status}"
                    )));
                }
            }
        }
        let mut statement = conn.prepare(&format!("SELECT {RECORD_COLUMNS} FROM retirement_cleanup WHERE status <> 'done' ORDER BY retirement_id, projection LIMIT ?1"))?;
        summary.samples = statement
            .query_map(
                [sample_limit.min(MAX_SUMMARY_SAMPLES)],
                cleanup_record_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        summary.omitted_components = total.saturating_sub(summary.samples.len() as u64);
        Ok(summary)
    }

    fn attempt_retirement_cleanup(
        &self,
        retirement_id: i64,
        projection: RetirementProjection,
        expected_backend: &str,
        cleanup: &mut dyn FnMut(&str) -> Result<(), StoreError>,
    ) -> Result<RetirementCleanupOutcome, StoreError> {
        let conn = self.conn.lock().map_err(|_| {
            StoreError::Compatibility(
                "store lock poisoned while attempting retirement cleanup".to_owned(),
            )
        })?;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        let record = tx.query_row(
            &format!("SELECT {RECORD_COLUMNS} FROM retirement_cleanup WHERE retirement_id = ?1 AND projection = ?2"),
            params![retirement_id, projection.as_str()], cleanup_record_from_row,
        ).optional()?.ok_or_else(|| StoreError::Compatibility(format!("retirement cleanup component missing: {retirement_id}/{}", projection.as_str())))?;
        match record.status {
            RetirementCleanupStatus::Done => return Ok(RetirementCleanupOutcome::AlreadyCompleted),
            RetirementCleanupStatus::SupersededByReintroduction => {
                return Ok(RetirementCleanupOutcome::SupersededByReintroduction);
            }
            RetirementCleanupStatus::LegacyUnknown => {
                return Ok(RetirementCleanupOutcome::LegacyUnknown);
            }
            RetirementCleanupStatus::Pending => {}
        }
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM symbols WHERE id = ?1)",
            [&record.symbol_id],
            |row| row.get(0),
        )?;
        if active {
            tx.execute("UPDATE retirement_cleanup SET status = 'superseded_by_reintroduction', last_error = NULL, updated_at = ?3 WHERE retirement_id = ?1 AND projection = ?2",
                params![retirement_id, projection.as_str(), time::current_unix_timestamp_secs()])?;
            tx.commit()?;
            return Ok(RetirementCleanupOutcome::SupersededByReintroduction);
        }
        let expected_backend =
            if projection == RetirementProjection::Graph && expected_backend == "cozo" {
                "surreal"
            } else {
                expected_backend
            };
        let result = if record.backend != expected_backend {
            Err(StoreError::Compatibility(format!(
                "retirement cleanup backend mismatch: recorded {}, current {expected_backend}",
                record.backend
            )))
        } else {
            cleanup(&record.symbol_id)
        };
        let error = result
            .as_ref()
            .err()
            .map(|error| error.to_string().chars().take(2048).collect::<String>());
        tx.execute("UPDATE retirement_cleanup SET status = ?3, attempts = attempts + 1, last_error = ?4, updated_at = ?5 WHERE retirement_id = ?1 AND projection = ?2",
            params![retirement_id, projection.as_str(), if result.is_ok() { "done" } else { "pending" }, error, time::current_unix_timestamp_secs()])?;
        tx.commit()?;
        result?;
        Ok(RetirementCleanupOutcome::Completed)
    }
}
