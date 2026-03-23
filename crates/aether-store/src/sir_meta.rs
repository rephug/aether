use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct SirMetaRecord {
    pub id: String,
    pub sir_hash: String,
    pub sir_version: i64,
    pub provider: String,
    pub model: String,
    pub generation_pass: String,
    pub reasoning_trace: Option<String>,
    pub prompt_hash: Option<String>,
    pub staleness_score: Option<f64>,
    pub updated_at: i64,
    pub sir_status: String,
    pub last_error: Option<String>,
    pub last_attempt_at: i64,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SirRowState {
    pub(crate) sir_hash: String,
    pub(crate) sir_version: i64,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) generation_pass: String,
    pub(crate) reasoning_trace: Option<String>,
    pub(crate) prompt_hash: Option<String>,
    pub(crate) staleness_score: Option<f64>,
    pub(crate) updated_at: i64,
    pub(crate) sir_status: String,
    pub(crate) last_error: Option<String>,
    pub(crate) last_attempt_at: i64,
    pub(crate) sir_json: Option<String>,
}
pub(crate) fn load_sir_row_state(
    tx: &Transaction<'_>,
    symbol_id: &str,
) -> Result<Option<SirRowState>, StoreError> {
    tx.query_row(
        r#"
        SELECT
            sir_hash,
            sir_version,
            provider,
            model,
            generation_pass,
            reasoning_trace,
            prompt_hash,
            staleness_score,
            updated_at,
            sir_status,
            last_error,
            last_attempt_at,
            sir_json
        FROM sir
        WHERE id = ?1
        "#,
        params![symbol_id],
        |row| {
            Ok(SirRowState {
                sir_hash: row.get(0)?,
                sir_version: row.get::<_, i64>(1)?.max(1),
                provider: row.get(2)?,
                model: row.get(3)?,
                generation_pass: row
                    .get::<_, Option<String>>(4)?
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "scan".to_owned()),
                reasoning_trace: row.get(5)?,
                prompt_hash: row.get(6)?,
                staleness_score: row.get(7)?,
                updated_at: row.get::<_, i64>(8)?.max(0),
                sir_status: row
                    .get::<_, Option<String>>(9)?
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| "fresh".to_owned()),
                last_error: row.get(10)?,
                last_attempt_at: row.get::<_, i64>(11)?.max(0),
                sir_json: row
                    .get::<_, Option<String>>(12)?
                    .filter(|value| !value.trim().is_empty()),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
pub(crate) fn upsert_sir_row_state(
    tx: &Transaction<'_>,
    symbol_id: &str,
    row: &SirRowState,
    sir_version: i64,
) -> Result<(), StoreError> {
    tx.execute(
        r#"
        INSERT INTO sir (
            id, sir_hash, sir_version, provider, model, generation_pass, reasoning_trace,
            prompt_hash, staleness_score, updated_at, sir_status, last_error, last_attempt_at,
            sir_json
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
        ON CONFLICT(id) DO UPDATE SET
            sir_hash = excluded.sir_hash,
            sir_version = excluded.sir_version,
            provider = excluded.provider,
            model = excluded.model,
            generation_pass = excluded.generation_pass,
            reasoning_trace = excluded.reasoning_trace,
            prompt_hash = excluded.prompt_hash,
            staleness_score = excluded.staleness_score,
            updated_at = excluded.updated_at,
            sir_status = excluded.sir_status,
            last_error = excluded.last_error,
            last_attempt_at = excluded.last_attempt_at,
            sir_json = excluded.sir_json
        "#,
        params![
            symbol_id,
            &row.sir_hash,
            sir_version.max(1),
            &row.provider,
            &row.model,
            &row.generation_pass,
            &row.reasoning_trace,
            &row.prompt_hash,
            row.staleness_score,
            row.updated_at,
            &row.sir_status,
            &row.last_error,
            row.last_attempt_at,
            &row.sir_json,
        ],
    )?;
    Ok(())
}

impl SqliteStore {
    pub fn list_sir_blobs_for_ids(
        &self,
        symbol_ids: &[String],
    ) -> Result<HashMap<String, String>, StoreError> {
        let normalized = symbol_ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if normalized.is_empty() {
            return Ok(HashMap::new());
        }

        let mut blobs = HashMap::new();
        let conn = self.conn.lock().unwrap();
        for chunk in normalized.chunks(SQLITE_PARAM_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                r#"
                SELECT id, sir_json
                FROM sir
                WHERE id IN ({placeholders})
                  AND COALESCE(TRIM(sir_json), '') <> ''
                ORDER BY id ASC
                "#
            );
            let params_vec = chunk
                .iter()
                .cloned()
                .map(SqlValue::Text)
                .collect::<Vec<_>>();
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(params_from_iter(params_vec), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (symbol_id, sir_json) = row?;
                blobs.insert(symbol_id, sir_json);
            }
        }

        Ok(blobs)
    }

    pub(crate) fn sir_blob_path(&self, symbol_id: &str) -> PathBuf {
        self.sir_dir.join(format!("{symbol_id}.json"))
    }
    fn upsert_sir_json_only(
        &self,
        symbol_id: &str,
        sir_json_string: &str,
    ) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO sir (id, sir_hash, sir_version, provider, model, updated_at, sir_json)
            VALUES (?1, '', 1, '', '', unixepoch(), ?2)
            ON CONFLICT(id) DO UPDATE SET
                sir_json = excluded.sir_json
            "#,
            params![symbol_id, sir_json_string],
        )?;

        Ok(())
    }
    fn read_sir_json_from_db(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT sir_json
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let json = stmt
            .query_row(params![symbol_id], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
            .filter(|value| !value.trim().is_empty());

        Ok(json)
    }
    pub fn list_symbol_ids_with_sir(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT s.id
            FROM symbols s
            JOIN sir r ON r.id = s.id
            WHERE COALESCE(TRIM(r.sir_json), '') <> ''
            ORDER BY s.id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn count_symbols_with_sir(&self) -> Result<(usize, usize), StoreError> {
        let conn = self.conn.lock().unwrap();
        let total = conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| {
            row.get::<_, i64>(0)
        })?;
        let with_sir = conn.query_row(
            r#"
            SELECT COUNT(DISTINCT s.id)
            FROM symbols s
            JOIN sir r ON r.id = s.id
            WHERE COALESCE(TRIM(r.sir_json), '') <> ''
            "#,
            [],
            |row| row.get::<_, i64>(0),
        )?;
        Ok((total.max(0) as usize, with_sir.max(0) as usize))
    }
    pub fn list_symbol_ids_without_sir(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT s.id
            FROM symbols s
            LEFT JOIN sir r ON r.id = s.id
            WHERE COALESCE(TRIM(r.sir_json), '') = ''
            ORDER BY s.id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn enqueue_sir_request(&self, symbol_id: &str) -> Result<(), StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO sir_requests (symbol_id, requested_at, request_count)
            VALUES (?1, unixepoch(), 1)
            ON CONFLICT(symbol_id) DO UPDATE SET
                requested_at = excluded.requested_at,
                request_count = sir_requests.request_count + 1
            "#,
            params![symbol_id],
        )?;
        Ok(())
    }
    pub fn list_sir_request_symbol_ids(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT symbol_id
            FROM sir_requests
            ORDER BY requested_at ASC, symbol_id ASC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit.max(1) as i64], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn consume_sir_requests(&self, limit: usize) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        let mut stmt = tx.prepare(
            r#"
            SELECT symbol_id
            FROM sir_requests
            ORDER BY requested_at ASC, symbol_id ASC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit.max(1) as i64], |row| row.get::<_, String>(0))?;
        let ids = rows.collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        if !ids.is_empty() {
            let mut delete = tx.prepare("DELETE FROM sir_requests WHERE symbol_id = ?1")?;
            for symbol_id in &ids {
                delete.execute(params![symbol_id])?;
            }
        }
        tx.commit()?;
        Ok(ids)
    }
    pub fn clear_sir_request(&self, symbol_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM sir_requests WHERE symbol_id = ?1",
            params![symbol_id.trim()],
        )?;
        Ok(())
    }
    pub(crate) fn store_write_sir_blob(
        &self,
        symbol_id: &str,
        sir_json_string: &str,
    ) -> Result<(), StoreError> {
        self.upsert_sir_json_only(symbol_id, sir_json_string)?;

        if self.mirror_sir_files {
            let path = self.sir_blob_path(symbol_id);
            if let Err(err) = fs::write(path, sir_json_string) {
                tracing::warn!(
                    symbol_id = %symbol_id,
                    error = %err,
                    "aether-store mirror write failed"
                );
            }
        }

        Ok(())
    }
    pub(crate) fn store_read_sir_blob(
        &self,
        symbol_id: &str,
    ) -> Result<Option<String>, StoreError> {
        if let Some(json) = self.read_sir_json_from_db(symbol_id)? {
            return Ok(Some(json));
        }

        let path = self.sir_blob_path(symbol_id);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(path)?;
        self.upsert_sir_json_only(symbol_id, &content)?;
        Ok(Some(content))
    }
    pub(crate) fn store_upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO sir (
                id, sir_hash, sir_version, provider, model, generation_pass, reasoning_trace,
                prompt_hash, staleness_score, updated_at, sir_status, last_error, last_attempt_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                sir_version = excluded.sir_version,
                provider = excluded.provider,
                model = excluded.model,
                generation_pass = excluded.generation_pass,
                reasoning_trace = excluded.reasoning_trace,
                prompt_hash = excluded.prompt_hash,
                staleness_score = excluded.staleness_score,
                updated_at = excluded.updated_at,
                sir_status = excluded.sir_status,
                last_error = excluded.last_error,
                last_attempt_at = excluded.last_attempt_at
            "#,
            params![
                record.id,
                record.sir_hash,
                record.sir_version,
                record.provider,
                record.model,
                record.generation_pass,
                record.reasoning_trace,
                record.prompt_hash,
                record.staleness_score,
                record.updated_at,
                record.sir_status,
                record.last_error,
                record.last_attempt_at,
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_get_sir_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirMetaRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                id,
                sir_hash,
                sir_version,
                provider,
                model,
                generation_pass,
                reasoning_trace,
                prompt_hash,
                staleness_score,
                updated_at,
                sir_status,
                last_error,
                last_attempt_at
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SirMetaRecord {
                    id: row.get(0)?,
                    sir_hash: row.get(1)?,
                    sir_version: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    generation_pass: row
                        .get::<_, Option<String>>(5)?
                        .map(|value| value.trim().to_owned())
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "scan".to_owned()),
                    reasoning_trace: row.get(6)?,
                    prompt_hash: row.get(7)?,
                    staleness_score: row.get(8)?,
                    updated_at: row.get(9)?,
                    sir_status: row.get(10)?,
                    last_error: row.get(11)?,
                    last_attempt_at: row.get(12)?,
                })
            })
            .optional()?;

        Ok(record)
    }
}
