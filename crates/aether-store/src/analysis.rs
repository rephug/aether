use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouplingMiningStateRecord {
    pub last_commit_hash: Option<String>,
    pub last_mined_at: Option<i64>,
    pub commits_scanned: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftAnalysisStateRecord {
    pub last_analysis_commit: Option<String>,
    pub last_analysis_at: Option<i64>,
    pub symbols_analyzed: i64,
    pub drift_detected: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftResultRecord {
    pub result_id: String,
    pub symbol_id: String,
    pub file_path: String,
    pub symbol_name: String,
    pub drift_type: String,
    pub drift_magnitude: Option<f32>,
    pub current_sir_hash: Option<String>,
    pub baseline_sir_hash: Option<String>,
    pub commit_range_start: Option<String>,
    pub commit_range_end: Option<String>,
    pub drift_summary: Option<String>,
    pub detail_json: String,
    pub detected_at: i64,
    pub is_acknowledged: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunitySnapshotRecord {
    pub snapshot_id: String,
    pub symbol_id: String,
    pub community_id: i64,
    pub captured_at: i64,
}
fn drift_result_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DriftResultRecord> {
    Ok(DriftResultRecord {
        result_id: row.get(0)?,
        symbol_id: row.get(1)?,
        file_path: row.get(2)?,
        symbol_name: row.get(3)?,
        drift_type: row.get(4)?,
        drift_magnitude: row.get(5)?,
        current_sir_hash: row.get(6)?,
        baseline_sir_hash: row.get(7)?,
        commit_range_start: row.get(8)?,
        commit_range_end: row.get(9)?,
        drift_summary: row.get(10)?,
        detail_json: row.get(11)?,
        detected_at: row.get(12)?,
        is_acknowledged: row.get::<_, i64>(13)? != 0,
    })
}

impl SqliteStore {
    pub(crate) fn store_get_coupling_mining_state(
        &self,
    ) -> Result<Option<CouplingMiningStateRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT last_commit_hash, last_mined_at, commits_scanned
            FROM coupling_mining_state
            WHERE id = 1
            LIMIT 1
            "#,
        )?;

        stmt.query_row([], |row| {
            Ok(CouplingMiningStateRecord {
                last_commit_hash: row.get(0)?,
                last_mined_at: row.get(1)?,
                commits_scanned: row.get::<_, i64>(2)?.max(0),
            })
        })
        .optional()
        .map_err(Into::into)
    }
    pub(crate) fn store_upsert_coupling_mining_state(
        &self,
        state: CouplingMiningStateRecord,
    ) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO coupling_mining_state (id, last_commit_hash, last_mined_at, commits_scanned)
            VALUES (1, ?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                last_commit_hash = excluded.last_commit_hash,
                last_mined_at = excluded.last_mined_at,
                commits_scanned = excluded.commits_scanned
            "#,
            params![
                state.last_commit_hash,
                state.last_mined_at.map(|value| value.max(0)),
                state.commits_scanned.max(0),
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_get_drift_analysis_state(
        &self,
    ) -> Result<Option<DriftAnalysisStateRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT last_analysis_commit, last_analysis_at, symbols_analyzed, drift_detected
            FROM drift_analysis_state
            WHERE id = 1
            LIMIT 1
            "#,
        )?;

        stmt.query_row([], |row| {
            Ok(DriftAnalysisStateRecord {
                last_analysis_commit: row.get(0)?,
                last_analysis_at: row.get(1)?,
                symbols_analyzed: row.get::<_, i64>(2)?.max(0),
                drift_detected: row.get::<_, i64>(3)?.max(0),
            })
        })
        .optional()
        .map_err(Into::into)
    }
    pub(crate) fn store_upsert_drift_analysis_state(
        &self,
        state: DriftAnalysisStateRecord,
    ) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO drift_analysis_state (
                id, last_analysis_commit, last_analysis_at, symbols_analyzed, drift_detected
            )
            VALUES (1, ?1, ?2, ?3, ?4)
            ON CONFLICT(id) DO UPDATE SET
                last_analysis_commit = excluded.last_analysis_commit,
                last_analysis_at = excluded.last_analysis_at,
                symbols_analyzed = excluded.symbols_analyzed,
                drift_detected = excluded.drift_detected
            "#,
            params![
                state
                    .last_analysis_commit
                    .map(|value| value.trim().to_ascii_lowercase()),
                state.last_analysis_at.map(|value| value.max(0)),
                state.symbols_analyzed.max(0),
                state.drift_detected.max(0),
            ],
        )?;
        Ok(())
    }
    pub(crate) fn store_upsert_drift_results(
        &self,
        records: &[DriftResultRecord],
    ) -> Result<(), StoreError> {
        if records.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO drift_results (
                    result_id, symbol_id, file_path, symbol_name, drift_type, drift_magnitude,
                    current_sir_hash, baseline_sir_hash, commit_range_start, commit_range_end,
                    drift_summary, detail_json, detected_at, is_acknowledged
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                ON CONFLICT(result_id) DO UPDATE SET
                    symbol_id = excluded.symbol_id,
                    file_path = excluded.file_path,
                    symbol_name = excluded.symbol_name,
                    drift_type = excluded.drift_type,
                    drift_magnitude = excluded.drift_magnitude,
                    current_sir_hash = excluded.current_sir_hash,
                    baseline_sir_hash = excluded.baseline_sir_hash,
                    commit_range_start = excluded.commit_range_start,
                    commit_range_end = excluded.commit_range_end,
                    drift_summary = excluded.drift_summary,
                    detail_json = excluded.detail_json,
                    detected_at = excluded.detected_at,
                    is_acknowledged = CASE
                        WHEN drift_results.is_acknowledged = 1 THEN 1
                        ELSE excluded.is_acknowledged
                    END
                "#,
            )?;

            for record in records {
                if record.result_id.trim().is_empty()
                    || record.symbol_id.trim().is_empty()
                    || record.file_path.trim().is_empty()
                    || record.symbol_name.trim().is_empty()
                    || record.drift_type.trim().is_empty()
                    || record.detail_json.trim().is_empty()
                {
                    continue;
                }
                stmt.execute(params![
                    record.result_id.trim(),
                    record.symbol_id.trim(),
                    normalize_path(record.file_path.trim()),
                    record.symbol_name.trim(),
                    record.drift_type.trim(),
                    record.drift_magnitude,
                    record.current_sir_hash.as_deref().map(str::trim),
                    record.baseline_sir_hash.as_deref().map(str::trim),
                    record.commit_range_start.as_deref().map(str::trim),
                    record.commit_range_end.as_deref().map(str::trim),
                    record
                        .drift_summary
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                    record.detail_json.trim(),
                    record.detected_at.max(0),
                    if record.is_acknowledged { 1 } else { 0 },
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn store_list_drift_results(
        &self,
        include_acknowledged: bool,
    ) -> Result<Vec<DriftResultRecord>, StoreError> {
        let sql = if include_acknowledged {
            r#"
            SELECT
                result_id, symbol_id, file_path, symbol_name, drift_type, drift_magnitude,
                current_sir_hash, baseline_sir_hash, commit_range_start, commit_range_end,
                drift_summary, detail_json, detected_at, is_acknowledged
            FROM drift_results
            ORDER BY detected_at DESC, result_id ASC
            "#
        } else {
            r#"
            SELECT
                result_id, symbol_id, file_path, symbol_name, drift_type, drift_magnitude,
                current_sir_hash, baseline_sir_hash, commit_range_start, commit_range_end,
                drift_summary, detail_json, detected_at, is_acknowledged
            FROM drift_results
            WHERE is_acknowledged = 0
            ORDER BY detected_at DESC, result_id ASC
            "#
        };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], drift_result_from_row)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }
    pub(crate) fn store_list_drift_results_by_ids(
        &self,
        result_ids: &[String],
    ) -> Result<Vec<DriftResultRecord>, StoreError> {
        let normalized = result_ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let mut records = Vec::new();
        let conn = self.conn.lock().unwrap();
        for chunk in normalized.chunks(SQLITE_PARAM_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                r#"
                SELECT
                    result_id, symbol_id, file_path, symbol_name, drift_type, drift_magnitude,
                    current_sir_hash, baseline_sir_hash, commit_range_start, commit_range_end,
                    drift_summary, detail_json, detected_at, is_acknowledged
                FROM drift_results
                WHERE result_id IN ({placeholders})
                ORDER BY detected_at DESC, result_id ASC
                "#
            );

            let params_vec = chunk
                .iter()
                .cloned()
                .map(SqlValue::Text)
                .collect::<Vec<_>>();
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(params_from_iter(params_vec), drift_result_from_row)?;
            for row in rows {
                records.push(row?);
            }
        }
        records.sort_by(|left, right| {
            right
                .detected_at
                .cmp(&left.detected_at)
                .then_with(|| left.result_id.cmp(&right.result_id))
        });
        records.dedup_by(|left, right| left.result_id == right.result_id);
        Ok(records)
    }
    pub(crate) fn store_acknowledge_drift_results(
        &self,
        result_ids: &[String],
    ) -> Result<u32, StoreError> {
        let normalized = result_ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if normalized.is_empty() {
            return Ok(0);
        }

        let mut changed = 0usize;
        let conn = self.conn.lock().unwrap();
        for chunk in normalized.chunks(SQLITE_PARAM_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE drift_results SET is_acknowledged = 1 WHERE result_id IN ({placeholders})"
            );
            let params_vec = chunk
                .iter()
                .cloned()
                .map(SqlValue::Text)
                .collect::<Vec<_>>();
            changed += conn.execute(sql.as_str(), params_from_iter(params_vec))?;
        }
        Ok(changed as u32)
    }
    pub(crate) fn store_replace_community_snapshot(
        &self,
        snapshot_id: &str,
        captured_at: i64,
        assignments: &[CommunitySnapshotRecord],
    ) -> Result<(), StoreError> {
        let snapshot_id = snapshot_id.trim().to_ascii_lowercase();
        if snapshot_id.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM community_snapshot WHERE snapshot_id = ?1",
            params![snapshot_id],
        )?;

        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO community_snapshot (snapshot_id, symbol_id, community_id, captured_at)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(snapshot_id, symbol_id) DO UPDATE SET
                    community_id = excluded.community_id,
                    captured_at = excluded.captured_at
                "#,
            )?;
            for assignment in assignments {
                let symbol_id = assignment.symbol_id.trim();
                if symbol_id.is_empty() {
                    continue;
                }
                stmt.execute(params![
                    snapshot_id,
                    symbol_id,
                    assignment.community_id,
                    captured_at.max(0),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn store_list_latest_community_snapshot(
        &self,
    ) -> Result<Vec<CommunitySnapshotRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let latest_snapshot_id = conn
            .query_row(
                r#"
                SELECT snapshot_id
                FROM community_snapshot
                ORDER BY captured_at DESC, snapshot_id DESC
                LIMIT 1
                "#,
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(snapshot_id) = latest_snapshot_id else {
            return Ok(Vec::new());
        };

        let mut stmt = conn.prepare(
            r#"
            SELECT snapshot_id, symbol_id, community_id, captured_at
            FROM community_snapshot
            WHERE snapshot_id = ?1
            ORDER BY symbol_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![snapshot_id], |row| {
            Ok(CommunitySnapshotRecord {
                snapshot_id: row.get(0)?,
                symbol_id: row.get(1)?,
                community_id: row.get(2)?,
                captured_at: row.get(3)?,
            })
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }
}
