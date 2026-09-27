use super::*;
use crate::sir_history::record_sir_version_if_changed_tx;
use crate::write_intents::update_intent_status_tx;

/// The identity of one stored SIR write: its content hash, the history version the
/// store assigned to it, and the row's write generation. A hash alone does not identify
/// a write, because a symbol's SIR can cycle back to an earlier content (`H1 → H2 → H1`)
/// through two injections; the history version only ever grows, but it is reused when
/// the same canonical content is written again (a forced injection, a metadata
/// promotion), so the write generation, which advances on every accepted write, tells
/// those apart too. Writers that plan a write against the SIR they observed compare
/// the whole triple, under the inject lock, right before they write.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SirIdentity {
    pub sir_hash: String,
    pub sir_version: i64,
    #[serde(default)]
    pub write_generation: i64,
}

/// A symbol's SIR row read in one query: metadata, write identity and stored JSON, so
/// a caller that builds a prompt or an enrichment from the blob and records the
/// identity for a later compare-and-set cannot pair one write's JSON with another
/// write's identity.
#[derive(Debug, Clone, PartialEq)]
pub struct SirRowSnapshot {
    pub meta: SirMetaRecord,
    pub identity: SirIdentity,
    pub blob: Option<String>,
}

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
    /// The content hash of the text the SIR describes, when its writer recorded it;
    /// it moves with the SIR when reconciliation migrates it to another id.
    pub(crate) source_hash: Option<String>,
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
            sir_json,
            source_hash
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
                source_hash: row.get(13)?,
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
            sir_json, source_hash
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
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
            sir_json = excluded.sir_json,
            source_hash = excluded.source_hash
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
            &row.source_hash,
        ],
    )?;
    Ok(())
}
fn upsert_sir_json_only_tx(
    tx: &Transaction<'_>,
    symbol_id: &str,
    sir_json_string: &str,
) -> Result<(), StoreError> {
    tx.execute(
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
fn upsert_sir_meta_tx(tx: &Transaction<'_>, record: &SirMetaRecord) -> Result<(), StoreError> {
    tx.execute(
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
            record.id.as_str(),
            record.sir_hash.as_str(),
            record.sir_version.max(1),
            record.provider.as_str(),
            record.model.as_str(),
            record.generation_pass.as_str(),
            record.reasoning_trace.as_deref(),
            record.prompt_hash.as_deref(),
            record.staleness_score,
            record.updated_at.max(0),
            record.sir_status.as_str(),
            record.last_error.as_deref(),
            record.last_attempt_at.max(0),
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

        let Some(content) = self.read_legacy_mirror(symbol_id)? else {
            return Ok(None);
        };
        self.upsert_sir_json_only(symbol_id, &content)?;
        Ok(Some(content))
    }

    /// The legacy file mirror of a symbol's SIR JSON, if one exists (`Ok(None)` when the
    /// file is absent, also when it disappears between the existence check and the
    /// read). Any other failure names the mirror path and the symbol, so a caller can
    /// tell which file of which symbol could not be read.
    fn read_legacy_mirror(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        let path = self.sir_blob_path(symbol_id);
        match fs::read_to_string(&path) {
            Ok(content) => Ok(Some(content)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(StoreError::Io(std::io::Error::new(
                err.kind(),
                format!(
                    "failed to read the legacy SIR mirror {} for symbol {symbol_id}: {err}",
                    path.display()
                ),
            ))),
        }
    }
    pub(crate) fn store_upsert_sir_meta(&self, record: SirMetaRecord) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        upsert_sir_meta_tx(&tx, &record)?;
        tx.commit()?;
        Ok(())
    }
    pub fn persist_sir_state_atomically(
        &self,
        record: SirMetaRecord,
        sir_json_string: &str,
        commit_hash: Option<&str>,
        write_intent_id: Option<&str>,
    ) -> Result<SirVersionWriteResult, StoreError> {
        self.persist_sir_state_atomically_with_source(
            record,
            sir_json_string,
            commit_hash,
            write_intent_id,
            None,
        )
    }

    /// `persist_sir_state_atomically` that also records, in the same transaction, the
    /// content hash of the symbol source this SIR describes (`sir.source_hash`). Every
    /// leaf write sets the column: to the hash when the writer knew which text the SIR
    /// was written for, to `NULL` otherwise, so the column never outlives the write it
    /// belongs to.
    pub fn persist_sir_state_atomically_with_source(
        &self,
        mut record: SirMetaRecord,
        sir_json_string: &str,
        commit_hash: Option<&str>,
        write_intent_id: Option<&str>,
        source_hash: Option<&str>,
    ) -> Result<SirVersionWriteResult, StoreError> {
        let symbol_id = record.id.trim();
        if symbol_id.is_empty() {
            return Err(StoreError::Compatibility(
                "SIR metadata id must be non-empty".to_owned(),
            ));
        }
        record.id = symbol_id.to_owned();

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        let current_json =
            load_sir_row_state(&tx, record.id.as_str())?.and_then(|row| row.sir_json);
        let write_result = record_sir_version_if_changed_tx(
            &tx,
            record.id.as_str(),
            record.sir_hash.as_str(),
            record.provider.as_str(),
            record.model.as_str(),
            sir_json_string,
            record.updated_at,
            commit_hash,
        )?;
        let should_write_json = current_json.as_deref() != Some(sir_json_string);
        if should_write_json {
            upsert_sir_json_only_tx(&tx, record.id.as_str(), sir_json_string)?;
        }
        record.sir_version = write_result.version;
        record.updated_at = write_result.updated_at;
        upsert_sir_meta_tx(&tx, &record)?;
        // Every accepted leaf write advances the row's write generation, so a writer
        // that observed the row before this write can tell, even when the content
        // hash and history version are unchanged.
        tx.execute(
            "UPDATE sir SET write_generation = write_generation + 1, source_hash = ?2 WHERE id = ?1",
            params![record.id.as_str(), source_hash],
        )?;
        if let Some(intent_id) = write_intent_id {
            update_intent_status_tx(&tx, intent_id, WriteIntentStatus::SqliteDone)?;
        }
        tx.commit()?;

        if should_write_json && self.mirror_sir_files {
            let path = self.sir_blob_path(record.id.as_str());
            if let Err(err) = fs::write(path, sir_json_string) {
                tracing::warn!(
                    symbol_id = %record.id,
                    error = %err,
                    "aether-store mirror write failed"
                );
            }
        }

        Ok(write_result)
    }
    /// The content hash of the symbol source the stored leaf describes, when its writer
    /// recorded one (see `persist_sir_state_atomically_with_source`).
    pub fn get_sir_source_hash(&self, symbol_id: &str) -> Result<Option<String>, StoreError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stmt = conn.prepare("SELECT source_hash FROM sir WHERE id = ?1")?;
        let mut rows = stmt.query(params![symbol_id.trim()])?;
        match rows.next()? {
            Some(row) => Ok(row.get::<_, Option<String>>(0)?),
            None => Ok(None),
        }
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
            .query_row(params![symbol_id], |row| sir_meta_from_row(row, 0))
            .optional()?;

        Ok(record)
    }

    /// A symbol's SIR row in one query (see [`SirRowSnapshot`]). A row migrated from
    /// the file-mirror era may hold its JSON only on disk; that blob is read from the
    /// mirror without writing it back, so this read never races a concurrent SIR writer.
    pub fn get_sir_meta_with_blob(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirRowSnapshot>, StoreError> {
        let Some(mut snapshot) = self.read_sir_row(symbol_id)? else {
            return Ok(None);
        };
        // The mirror file is written after the row's transaction commits, so a writer
        // landing between the row read and the file read would pair the older identity
        // with the newer JSON. The row is read again after the file: only when its
        // identity is unchanged do the two belong together; otherwise the newer row is
        // taken (and, once it holds its JSON itself, returned as is).
        for _ in 0..3 {
            if snapshot.blob.is_some() {
                return Ok(Some(snapshot));
            }
            let Some(mirrored) = self.read_legacy_mirror(symbol_id)? else {
                return Ok(Some(snapshot));
            };
            match self.read_sir_row(symbol_id)? {
                None => return Ok(None),
                Some(again) if again.identity == snapshot.identity => {
                    snapshot.blob = Some(mirrored).filter(|value| !value.trim().is_empty());
                    return Ok(Some(snapshot));
                }
                Some(again) => snapshot = again,
            }
        }
        Err(StoreError::Compatibility(format!(
            "SIR row for {symbol_id} kept changing while its legacy mirror was read"
        )))
    }

    /// The identity of the SIR a symbol holds right now (`None`: no SIR stored).
    pub fn get_sir_identity(&self, symbol_id: &str) -> Result<Option<SirIdentity>, StoreError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut stmt = conn.prepare(
            r#"
            SELECT sir_hash, sir_version, write_generation
            FROM sir
            WHERE id = ?1
            "#,
        )?;
        let identity = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SirIdentity {
                    sir_hash: row.get(0)?,
                    sir_version: row.get::<_, i64>(1)?.max(1),
                    write_generation: row.get(2)?,
                })
            })
            .optional()?;
        Ok(identity)
    }

    fn read_sir_row(&self, symbol_id: &str) -> Result<Option<SirRowSnapshot>, StoreError> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
                last_attempt_at,
                sir_json,
                write_generation
            FROM sir
            WHERE id = ?1
            "#,
        )?;

        let snapshot = stmt
            .query_row(params![symbol_id], |row| {
                let meta = sir_meta_from_row(row, 0)?;
                let identity = SirIdentity {
                    sir_hash: meta.sir_hash.clone(),
                    sir_version: meta.sir_version.max(1),
                    write_generation: row.get(14)?,
                };
                Ok(SirRowSnapshot {
                    meta,
                    identity,
                    blob: row
                        .get::<_, Option<String>>(13)?
                        .filter(|value| !value.trim().is_empty()),
                })
            })
            .optional()?;

        Ok(snapshot)
    }
}

/// Read a `SirMetaRecord` from the thirteen metadata columns starting at `offset`.
fn sir_meta_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<SirMetaRecord> {
    Ok(SirMetaRecord {
        id: row.get(offset)?,
        sir_hash: row.get(offset + 1)?,
        sir_version: row.get(offset + 2)?,
        provider: row.get(offset + 3)?,
        model: row.get(offset + 4)?,
        generation_pass: row
            .get::<_, Option<String>>(offset + 5)?
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "scan".to_owned()),
        reasoning_trace: row.get(offset + 6)?,
        prompt_hash: row.get(offset + 7)?,
        staleness_score: row.get(offset + 8)?,
        updated_at: row.get(offset + 9)?,
        sir_status: row.get(offset + 10)?,
        last_error: row.get(offset + 11)?,
        last_attempt_at: row.get(offset + 12)?,
    })
}
