//! What the store can say about its own freshness: when each file was last indexed and
//! which SIR rows are stale or can't be checked for staleness.

use std::sync::MutexGuard;

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
    /// When the indexer last started reading the file (unix milliseconds), if a binary
    /// that records it did.
    pub read_at_ms: Option<i64>,
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
    /// The connection, or [`StoreError::LockPoisoned`] when an operation panicked while
    /// holding it, so a diagnostic read reports the earlier failure instead of panicking.
    pub(crate) fn lock_connection(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.conn.lock().map_err(|_| {
            StoreError::LockPoisoned(
                "an earlier store operation panicked while holding the connection".to_owned(),
            )
        })
    }

    /// Every file with indexed symbols, ordered by path.
    pub fn list_indexed_files(&self) -> Result<Vec<IndexedFileRecord>, StoreError> {
        let conn = self.lock_connection()?;
        // A store opened read-only is not migrated, so the table may predate this binary.
        let read_at = if file_reads_table_exists(&conn)? {
            "(SELECT fr.read_at_ms FROM file_reads fr WHERE fr.file_path = s.file_path)"
        } else {
            "NULL"
        };
        let mut stmt = conn.prepare(&format!(
            r#"
            SELECT
                s.file_path,
                MAX(s.last_seen_at),
                COUNT(*),
                SUM(CASE WHEN COALESCE(TRIM(r.sir_json), '') <> '' THEN 1 ELSE 0 END),
                {read_at}
            FROM symbols s
            LEFT JOIN sir r ON r.id = s.id
            GROUP BY s.file_path
            ORDER BY s.file_path ASC
            "#
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok(IndexedFileRecord {
                file_path: row.get(0)?,
                last_indexed_at: row.get(1)?,
                symbol_count: row.get::<_, i64>(2)?.max(0) as u64,
                symbols_with_sir: row.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u64,
                read_at_ms: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// When the indexer last read `file_path`: the latest `last_seen_at` of its symbols.
    /// `None` when it has none.
    pub fn file_last_read_at(&self, file_path: &str) -> Result<Option<i64>, StoreError> {
        let conn = self.lock_connection()?;
        conn.query_row(
            "SELECT MAX(last_seen_at) FROM symbols WHERE file_path = ?1",
            params![file_path],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(Into::into)
    }

    /// Records that the indexer read the file declaring `symbol_ids` at `read_at` (unix
    /// seconds) and found each of them as stored. Only the symbols the file still declares
    /// are passed, so rows a full sync's reconciliation should remove keep their old time.
    pub fn mark_symbols_read(&self, symbol_ids: &[String], read_at: i64) -> Result<(), StoreError> {
        let mut conn = self.lock_connection()?;
        let tx = conn.transaction()?;
        {
            let mut stmt = tx
                .prepare("UPDATE symbols SET last_seen_at = MAX(last_seen_at, ?2) WHERE id = ?1")?;
            for symbol_id in symbol_ids {
                stmt.execute(params![symbol_id, read_at])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Records that the indexer started reading each of `file_paths` at `read_at_ms` (unix
    /// milliseconds), replacing any earlier time.
    pub fn record_file_reads(
        &self,
        file_paths: &[String],
        read_at_ms: i64,
    ) -> Result<(), StoreError> {
        if file_paths.is_empty() {
            return Ok(());
        }
        let conn = self.lock_connection()?;
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        {
            let mut upsert = tx.prepare(
                r#"
                INSERT INTO file_reads (file_path, read_at_ms)
                VALUES (?1, ?2)
                ON CONFLICT(file_path) DO UPDATE SET read_at_ms = excluded.read_at_ms
                "#,
            )?;
            for file_path in file_paths {
                upsert.execute(params![file_path, read_at_ms])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every recorded read start (unix milliseconds) by file path, symbol-free files
    /// included; empty when the store predates the table.
    pub fn list_file_reads(&self) -> Result<HashMap<String, i64>, StoreError> {
        let conn = self.lock_connection()?;
        if !file_reads_table_exists(&conn)? {
            return Ok(HashMap::new());
        }
        let mut stmt = conn.prepare("SELECT file_path, read_at_ms FROM file_reads")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<HashMap<_, _>, _>>()
            .map_err(Into::into)
    }

    /// When the indexer last started reading `file_path` (unix milliseconds), if recorded.
    pub fn file_read_at_ms(&self, file_path: &str) -> Result<Option<i64>, StoreError> {
        let conn = self.lock_connection()?;
        if !file_reads_table_exists(&conn)? {
            return Ok(None);
        }
        conn.query_row(
            "SELECT read_at_ms FROM file_reads WHERE file_path = ?1",
            params![file_path],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    pub fn sir_freshness_counts(&self) -> Result<SirFreshnessCounts, StoreError> {
        let conn = self.lock_connection()?;
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

fn file_reads_table_exists(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'file_reads'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use tempfile::tempdir;

    use crate::{SqliteStore, StoreError, SymbolCatalogStore, SymbolRecord};

    /// A panic while another operation holds the connection must not turn every later
    /// status read into a panic too.
    #[test]
    fn freshness_and_coverage_reads_report_a_poisoned_lock_as_an_error() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let _guard = store.conn.lock().expect("first lock");
            panic!("poison the connection lock");
        }));
        assert!(poisoned.is_err());

        assert!(matches!(
            store.list_indexed_files(),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.sir_freshness_counts(),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.file_last_read_at("src/lib.rs"),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.mark_symbols_read(&["caller".to_owned()], 1),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.call_edge_coverage_by_language(),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.call_edge_coverage_for_sources(&["caller".to_owned()]),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.record_file_reads(&["src/lib.rs".to_owned()], 1),
            Err(StoreError::LockPoisoned(_))
        ));
        assert!(matches!(
            store.file_read_at_ms("src/lib.rs"),
            Err(StoreError::LockPoisoned(_))
        ));
    }

    #[test]
    fn file_read_times_are_recorded_listed_and_replaced() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .upsert_symbol(SymbolRecord {
                id: "alpha".to_owned(),
                file_path: "src/lib.rs".to_owned(),
                language: "rust".to_owned(),
                kind: "function".to_owned(),
                qualified_name: "alpha".to_owned(),
                signature_fingerprint: "sig".to_owned(),
                last_seen_at: 7,
            })
            .expect("upsert symbol");
        let listed = || {
            store
                .list_indexed_files()
                .expect("list indexed files")
                .into_iter()
                .map(|file| file.read_at_ms)
                .collect::<Vec<_>>()
        };
        assert_eq!(listed(), [None]);

        store
            .record_file_reads(&["src/lib.rs".to_owned()], 7_250)
            .expect("record read");
        assert_eq!(listed(), [Some(7_250)]);
        store
            .record_file_reads(&["src/lib.rs".to_owned()], 9_000)
            .expect("record a later read");
        assert_eq!(
            store.file_read_at_ms("src/lib.rs").expect("read"),
            Some(9_000)
        );
        assert_eq!(store.file_read_at_ms("src/other.rs").expect("read"), None);
        // A file without symbols is listed too.
        store
            .record_file_reads(&["src/empty.rs".to_owned()], 9_500)
            .expect("record a symbol-free read");
        let reads = store.list_file_reads().expect("list reads");
        assert_eq!(reads.get("src/empty.rs"), Some(&9_500));
        assert_eq!(reads.get("src/lib.rs"), Some(&9_000));
    }

    /// A store last migrated by a binary that predates `file_reads` and opened read-only
    /// (so not migrated now) still lists its files, without read times.
    #[test]
    fn a_store_without_the_file_reads_table_lists_files_without_read_times() {
        let temp = tempdir().expect("tempdir");
        {
            let store = SqliteStore::open(temp.path()).expect("open store");
            store
                .upsert_symbol(SymbolRecord {
                    id: "alpha".to_owned(),
                    file_path: "src/lib.rs".to_owned(),
                    language: "rust".to_owned(),
                    kind: "function".to_owned(),
                    qualified_name: "alpha".to_owned(),
                    signature_fingerprint: "sig".to_owned(),
                    last_seen_at: 7,
                })
                .expect("upsert symbol");
        }
        rusqlite::Connection::open(temp.path().join(".aether/meta.sqlite"))
            .and_then(|conn| conn.execute("DROP TABLE file_reads", []))
            .expect("drop table");

        let store = SqliteStore::open_readonly(temp.path()).expect("open read-only");
        let files = store.list_indexed_files().expect("list indexed files");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].read_at_ms, None);
        assert_eq!(store.file_read_at_ms("src/lib.rs").expect("read"), None);
        assert!(store.list_file_reads().expect("list reads").is_empty());
    }
}
