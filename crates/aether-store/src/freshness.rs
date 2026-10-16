//! What the store can say about its own freshness: when each file was last indexed and
//! which SIR rows are stale or can't be checked for staleness.

use super::*;

/// One indexed file: when its symbols were last written, and how many it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFileRecord {
    pub file_path: String,
    /// The latest `last_seen_at` among the file's symbols (unix seconds): when the
    /// indexer last read the file.
    pub last_indexed_at: i64,
    pub symbol_count: u64,
    /// Symbols of the file with a non-empty SIR.
    pub symbols_with_sir: u64,
}

/// Counts of SIR rows by freshness. Leaves are the SIRs of indexed symbols; the other
/// rows are file and module rollups.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SirFreshnessCounts {
    pub leaf_rows: u64,
    /// Leaves marked `stale`: their symbol's source changed after they were written.
    pub stale_leaves: u64,
    /// Leaves without a recorded source hash. A change to their symbol can't be detected
    /// by comparing hashes, so it marks them stale only while a daemon watches the file.
    pub leaves_without_source_hash: u64,
    pub rollup_rows: u64,
    pub stale_rollups: u64,
    /// The latest `updated_at` of any SIR row (unix seconds).
    pub last_sir_update_at: Option<i64>,
}

impl SqliteStore {
    /// Every file with indexed symbols, ordered by path.
    pub fn list_indexed_files(&self) -> Result<Vec<IndexedFileRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.file_path,
                MAX(s.last_seen_at),
                COUNT(*),
                SUM(CASE WHEN COALESCE(TRIM(r.sir_json), '') <> '' THEN 1 ELSE 0 END)
            FROM symbols s
            LEFT JOIN sir r ON r.id = s.id
            GROUP BY s.file_path
            ORDER BY s.file_path ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(IndexedFileRecord {
                file_path: row.get(0)?,
                last_indexed_at: row.get(1)?,
                symbol_count: row.get::<_, i64>(2)?.max(0) as u64,
                symbols_with_sir: row.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u64,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn sir_freshness_counts(&self) -> Result<SirFreshnessCounts, StoreError> {
        let conn = self.conn.lock().unwrap();
        let (leaf_rows, stale_leaves, leaves_without_source_hash) = conn.query_row(
            r#"
            SELECT
                COUNT(*),
                SUM(CASE WHEN LOWER(TRIM(r.sir_status)) = 'stale' THEN 1 ELSE 0 END),
                SUM(CASE WHEN COALESCE(TRIM(r.source_hash), '') = '' THEN 1 ELSE 0 END)
            FROM sir r
            JOIN symbols s ON s.id = r.id
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            },
        )?;
        let (rollup_rows, stale_rollups, last_sir_update_at) = conn.query_row(
            r#"
            SELECT
                SUM(CASE WHEN s.id IS NULL THEN 1 ELSE 0 END),
                SUM(CASE WHEN s.id IS NULL AND LOWER(TRIM(r.sir_status)) = 'stale'
                    THEN 1 ELSE 0 END),
                MAX(r.updated_at)
            FROM sir r
            LEFT JOIN symbols s ON s.id = r.id
            "#,
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )?;
        let count = |value: i64| value.max(0) as u64;
        Ok(SirFreshnessCounts {
            leaf_rows: count(leaf_rows),
            stale_leaves: count(stale_leaves),
            leaves_without_source_hash: count(leaves_without_source_hash),
            rollup_rows: count(rollup_rows),
            stale_rollups: count(stale_rollups),
            last_sir_update_at,
        })
    }
}
