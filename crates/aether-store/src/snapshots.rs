use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub signature_fingerprint: String,
    pub sir_json: String,
    pub generation_pass: String,
    pub was_deep_scanned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSnapshotSummary {
    pub snapshot_id: String,
    pub git_commit: String,
    pub created_at: i64,
    pub scope: String,
    pub symbol_count: usize,
    pub deep_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentSnapshot {
    pub snapshot_id: String,
    pub git_commit: String,
    pub created_at: i64,
    pub scope: String,
    pub symbol_count: usize,
    pub deep_count: usize,
    pub symbols: Vec<SnapshotEntry>,
}

fn row_to_snapshot_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<IntentSnapshotSummary> {
    Ok(IntentSnapshotSummary {
        snapshot_id: row.get(0)?,
        git_commit: row.get(1)?,
        created_at: row.get::<_, i64>(2)?.max(0),
        scope: row.get(3)?,
        symbol_count: row.get::<_, i64>(4)?.max(0) as usize,
        deep_count: row.get::<_, i64>(5)?.max(0) as usize,
    })
}

fn row_to_snapshot_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotEntry> {
    Ok(SnapshotEntry {
        symbol_id: row.get(0)?,
        qualified_name: row.get(1)?,
        file_path: row.get(2)?,
        signature_fingerprint: row.get(3)?,
        sir_json: row.get(4)?,
        generation_pass: row.get(5)?,
        was_deep_scanned: row.get::<_, i64>(6)? != 0,
    })
}

fn normalize_generation_pass(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "scan".to_owned()
    } else {
        normalized
    }
}

fn normalize_snapshot_entry(entry: &SnapshotEntry) -> SnapshotEntry {
    SnapshotEntry {
        symbol_id: entry.symbol_id.trim().to_owned(),
        qualified_name: entry.qualified_name.trim().to_owned(),
        file_path: normalize_path(entry.file_path.trim()),
        signature_fingerprint: entry.signature_fingerprint.trim().to_owned(),
        sir_json: entry.sir_json.trim().to_owned(),
        generation_pass: normalize_generation_pass(entry.generation_pass.as_str()),
        was_deep_scanned: entry.was_deep_scanned,
    }
}

impl SqliteStore {
    pub(crate) fn store_create_snapshot(
        &self,
        snapshot: &IntentSnapshot,
    ) -> Result<(), StoreError> {
        let snapshot_id = snapshot.snapshot_id.trim();
        let git_commit = snapshot.git_commit.trim().to_ascii_lowercase();
        let scope = normalize_path(snapshot.scope.trim());
        if snapshot_id.is_empty() || git_commit.is_empty() || scope.is_empty() {
            return Err(StoreError::Compatibility(
                "snapshot_id, git_commit, and scope must be non-empty".to_owned(),
            ));
        }

        let mut entries = snapshot
            .symbols
            .iter()
            .map(normalize_snapshot_entry)
            .filter(|entry| {
                !entry.symbol_id.is_empty()
                    && !entry.qualified_name.is_empty()
                    && !entry.file_path.is_empty()
                    && !entry.signature_fingerprint.is_empty()
                    && !entry.sir_json.is_empty()
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            left.file_path
                .cmp(&right.file_path)
                .then_with(|| left.qualified_name.cmp(&right.qualified_name))
                .then_with(|| left.symbol_id.cmp(&right.symbol_id))
        });

        let symbol_count = entries.len() as i64;
        let deep_count = entries
            .iter()
            .filter(|entry| entry.was_deep_scanned)
            .count() as i64;
        let created_at = snapshot.created_at.max(0);

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        tx.execute(
            r#"
            INSERT INTO intent_snapshots (
                snapshot_id, git_commit, created_at, scope, symbol_count, deep_count
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(snapshot_id) DO UPDATE SET
                git_commit = excluded.git_commit,
                created_at = excluded.created_at,
                scope = excluded.scope,
                symbol_count = excluded.symbol_count,
                deep_count = excluded.deep_count
            "#,
            params![
                snapshot_id,
                git_commit,
                created_at,
                scope,
                symbol_count,
                deep_count
            ],
        )?;
        tx.execute(
            "DELETE FROM intent_snapshot_entries WHERE snapshot_id = ?1",
            params![snapshot_id],
        )?;

        if !entries.is_empty() {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO intent_snapshot_entries (
                    snapshot_id,
                    symbol_id,
                    qualified_name,
                    file_path,
                    signature_fingerprint,
                    sir_json,
                    generation_pass,
                    was_deep_scanned
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
            )?;

            for entry in entries {
                stmt.execute(params![
                    snapshot_id,
                    entry.symbol_id,
                    entry.qualified_name,
                    entry.file_path,
                    entry.signature_fingerprint,
                    entry.sir_json,
                    entry.generation_pass,
                    i64::from(entry.was_deep_scanned)
                ])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub(crate) fn store_get_snapshot(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<IntentSnapshot>, StoreError> {
        let snapshot_id = snapshot_id.trim();
        if snapshot_id.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        let summary = conn
            .query_row(
                r#"
                SELECT snapshot_id, git_commit, created_at, scope, symbol_count, deep_count
                FROM intent_snapshots
                WHERE snapshot_id = ?1
                "#,
                params![snapshot_id],
                row_to_snapshot_summary,
            )
            .optional()?;
        drop(conn);

        let Some(summary) = summary else {
            return Ok(None);
        };
        let symbols = self.store_get_snapshot_entries(snapshot_id)?;
        Ok(Some(IntentSnapshot {
            snapshot_id: summary.snapshot_id,
            git_commit: summary.git_commit,
            created_at: summary.created_at,
            scope: summary.scope,
            symbol_count: summary.symbol_count,
            deep_count: summary.deep_count,
            symbols,
        }))
    }

    pub(crate) fn store_list_snapshots(&self) -> Result<Vec<IntentSnapshotSummary>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT snapshot_id, git_commit, created_at, scope, symbol_count, deep_count
            FROM intent_snapshots
            ORDER BY created_at DESC, snapshot_id DESC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_snapshot_summary)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub(crate) fn store_get_snapshot_entries(
        &self,
        snapshot_id: &str,
    ) -> Result<Vec<SnapshotEntry>, StoreError> {
        let snapshot_id = snapshot_id.trim();
        if snapshot_id.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                symbol_id,
                qualified_name,
                file_path,
                signature_fingerprint,
                sir_json,
                generation_pass,
                was_deep_scanned
            FROM intent_snapshot_entries
            WHERE snapshot_id = ?1
            ORDER BY file_path ASC, qualified_name ASC, symbol_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![snapshot_id], row_to_snapshot_entry)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub(crate) fn store_delete_snapshot(&self, snapshot_id: &str) -> Result<(), StoreError> {
        let snapshot_id = snapshot_id.trim();
        if snapshot_id.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM intent_snapshot_entries WHERE snapshot_id = ?1",
            params![snapshot_id],
        )?;
        tx.execute(
            "DELETE FROM intent_snapshots WHERE snapshot_id = ?1",
            params![snapshot_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn sample_snapshot(snapshot_id: &str, symbol_ids: &[&str]) -> IntentSnapshot {
        IntentSnapshot {
            snapshot_id: snapshot_id.to_owned(),
            git_commit: "c334845".to_owned(),
            created_at: 1_710_000_000,
            scope: "crates/aether-mcp/src/lib.rs".to_owned(),
            symbol_count: symbol_ids.len(),
            deep_count: 1,
            symbols: symbol_ids
                .iter()
                .enumerate()
                .map(|(index, symbol_id)| SnapshotEntry {
                    symbol_id: (*symbol_id).to_owned(),
                    qualified_name: format!("demo::{symbol_id}"),
                    file_path: "src/lib.rs".to_owned(),
                    signature_fingerprint: format!("sig-{symbol_id}"),
                    sir_json: format!("{{\"intent\":\"{symbol_id}\"}}"),
                    generation_pass: if index == 0 { "deep" } else { "scan" }.to_owned(),
                    was_deep_scanned: index == 0,
                })
                .collect(),
        }
    }

    #[test]
    fn snapshot_round_trip_persists_header_and_entries() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        let snapshot = sample_snapshot("snap-1", &["sym-a", "sym-b"]);

        store.create_snapshot(&snapshot).expect("create snapshot");

        let loaded = store
            .get_snapshot("snap-1")
            .expect("read snapshot")
            .expect("snapshot exists");
        assert_eq!(loaded, snapshot);
    }

    #[test]
    fn list_snapshots_returns_expected_count() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .create_snapshot(&sample_snapshot("snap-1", &["sym-a"]))
            .expect("create first snapshot");
        store
            .create_snapshot(&sample_snapshot("snap-2", &["sym-b", "sym-c"]))
            .expect("create second snapshot");

        let snapshots = store.list_snapshots().expect("list snapshots");
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].snapshot_id, "snap-2");
        assert_eq!(snapshots[1].snapshot_id, "snap-1");
    }

    #[test]
    fn delete_snapshot_removes_header_and_entries() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");
        store
            .create_snapshot(&sample_snapshot("snap-1", &["sym-a", "sym-b"]))
            .expect("create snapshot");

        store.delete_snapshot("snap-1").expect("delete snapshot");

        assert!(
            store
                .get_snapshot("snap-1")
                .expect("get deleted snapshot")
                .is_none()
        );
        assert!(
            store
                .get_snapshot_entries("snap-1")
                .expect("get deleted entries")
                .is_empty()
        );
    }

    #[test]
    fn get_snapshot_entries_returns_empty_for_missing_snapshot() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        let entries = store
            .get_snapshot_entries("missing")
            .expect("get missing snapshot entries");
        assert!(entries.is_empty());
    }
}
