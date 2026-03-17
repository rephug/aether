use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct SirFingerprintHistoryRecord {
    pub symbol_id: String,
    pub timestamp: i64,
    pub prompt_hash: String,
    pub prompt_hash_previous: Option<String>,
    pub trigger: String,
    pub source_changed: bool,
    pub neighbor_changed: bool,
    pub config_changed: bool,
    pub generation_model: Option<String>,
    pub generation_pass: Option<String>,
    pub delta_sem: Option<f64>,
}

impl SqliteStore {
    pub fn insert_sir_fingerprint_history(
        &self,
        record: &SirFingerprintHistoryRecord,
    ) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO sir_fingerprint_history (
                symbol_id,
                timestamp,
                prompt_hash,
                prompt_hash_previous,
                trigger,
                source_changed,
                neighbor_changed,
                config_changed,
                generation_model,
                generation_pass,
                delta_sem
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                record.symbol_id,
                record.timestamp.max(0),
                record.prompt_hash,
                record.prompt_hash_previous,
                record.trigger,
                if record.source_changed { 1_i64 } else { 0_i64 },
                if record.neighbor_changed {
                    1_i64
                } else {
                    0_i64
                },
                if record.config_changed { 1_i64 } else { 0_i64 },
                record.generation_model,
                record.generation_pass,
                record.delta_sem,
            ],
        )?;
        Ok(())
    }

    pub fn list_sir_fingerprint_history(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<SirFingerprintHistoryRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                symbol_id,
                timestamp,
                prompt_hash,
                prompt_hash_previous,
                trigger,
                source_changed,
                neighbor_changed,
                config_changed,
                generation_model,
                generation_pass,
                delta_sem
            FROM sir_fingerprint_history
            WHERE symbol_id = ?1
            ORDER BY timestamp ASC, id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok(SirFingerprintHistoryRecord {
                symbol_id: row.get(0)?,
                timestamp: row.get(1)?,
                prompt_hash: row.get(2)?,
                prompt_hash_previous: row.get(3)?,
                trigger: row.get(4)?,
                source_changed: row.get::<_, i64>(5)? != 0,
                neighbor_changed: row.get::<_, i64>(6)? != 0,
                config_changed: row.get::<_, i64>(7)? != 0,
                generation_model: row.get(8)?,
                generation_pass: row.get(9)?,
                delta_sem: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return the most recent fingerprint changes across all symbols.
    pub fn list_recent_fingerprint_changes(
        &self,
        limit: usize,
    ) -> Result<Vec<SirFingerprintHistoryRecord>, StoreError> {
        let capped_limit = limit.clamp(1, 500) as i64;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                symbol_id,
                timestamp,
                prompt_hash,
                prompt_hash_previous,
                trigger,
                source_changed,
                neighbor_changed,
                config_changed,
                generation_model,
                generation_pass,
                delta_sem
            FROM sir_fingerprint_history
            ORDER BY timestamp DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![capped_limit], |row| {
            Ok(SirFingerprintHistoryRecord {
                symbol_id: row.get(0)?,
                timestamp: row.get(1)?,
                prompt_hash: row.get(2)?,
                prompt_hash_previous: row.get(3)?,
                trigger: row.get(4)?,
                source_changed: row.get::<_, i64>(5)? != 0,
                neighbor_changed: row.get::<_, i64>(6)? != 0,
                config_changed: row.get::<_, i64>(7)? != 0,
                generation_model: row.get(8)?,
                generation_pass: row.get(9)?,
                delta_sem: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Count fingerprint changes per symbol, returning the top N most-changed symbols.
    pub fn count_fingerprint_changes_by_symbol(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, usize)>, StoreError> {
        let capped_limit = limit.clamp(1, 200) as i64;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT symbol_id, COUNT(*) as cnt
            FROM sir_fingerprint_history
            GROUP BY symbol_id
            ORDER BY cnt DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![capped_limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
