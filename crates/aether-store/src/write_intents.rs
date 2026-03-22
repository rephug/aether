use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteIntent {
    pub intent_id: String,
    pub symbol_id: String,
    pub file_path: String,
    pub operation: IntentOperation,
    pub status: WriteIntentStatus,
    pub payload_json: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchCompleteResult {
    pub completed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteIntentStatus {
    Pending,
    SqliteDone,
    VectorDone,
    GraphDone,
    Complete,
    Failed,
}
impl WriteIntentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::SqliteDone => "sqlite_done",
            Self::VectorDone => "vector_done",
            Self::GraphDone => "graph_done",
            Self::Complete => "complete",
            Self::Failed => "failed",
        }
    }
}
impl fmt::Display for WriteIntentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for WriteIntentStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "sqlite_done" => Ok(Self::SqliteDone),
            "vector_done" => Ok(Self::VectorDone),
            "graph_done" => Ok(Self::GraphDone),
            "complete" => Ok(Self::Complete),
            "failed" => Ok(Self::Failed),
            other => Err(format!("invalid write intent status '{other}'")),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentOperation {
    UpsertSir,
    DeleteSymbol,
    UpdateEdges,
}
impl IntentOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UpsertSir => "upsert_sir",
            Self::DeleteSymbol => "delete_symbol",
            Self::UpdateEdges => "update_edges",
        }
    }
}
impl fmt::Display for IntentOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for IntentOperation {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "upsert_sir" => Ok(Self::UpsertSir),
            "delete_symbol" => Ok(Self::DeleteSymbol),
            "update_edges" => Ok(Self::UpdateEdges),
            other => Err(format!("invalid write intent operation '{other}'")),
        }
    }
}
fn write_intent_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WriteIntent> {
    let operation_raw = row.get::<_, String>(3)?;
    let status_raw = row.get::<_, String>(4)?;
    let operation = IntentOperation::from_str(operation_raw.as_str()).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
        )
    })?;
    let status = WriteIntentStatus::from_str(status_raw.as_str()).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, err)),
        )
    })?;

    Ok(WriteIntent {
        intent_id: row.get(0)?,
        symbol_id: row.get(1)?,
        file_path: row.get(2)?,
        operation,
        status,
        payload_json: row.get(5)?,
        created_at: row.get(6)?,
        completed_at: row.get(7)?,
        error_message: row.get(8)?,
    })
}

impl SqliteStore {
    pub fn create_write_intent(&self, intent: &WriteIntent) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO write_intents (
                intent_id,
                symbol_id,
                file_path,
                operation,
                status,
                payload_json,
                created_at,
                completed_at,
                error_message
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                intent.intent_id.as_str(),
                intent.symbol_id.as_str(),
                intent.file_path.as_str(),
                intent.operation.to_string(),
                intent.status.to_string(),
                intent.payload_json.as_deref(),
                intent.created_at.max(0),
                intent.completed_at.map(|value| value.max(0)),
                intent.error_message.as_deref(),
            ],
        )?;
        Ok(())
    }
    pub fn update_intent_status(
        &self,
        intent_id: &str,
        status: WriteIntentStatus,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE write_intents
            SET status = ?2
            WHERE intent_id = ?1
            "#,
            params![intent_id, status.to_string()],
        )?;
        Ok(())
    }
    pub fn mark_intent_failed(&self, intent_id: &str, error: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE write_intents
            SET status = ?2,
                error_message = ?3,
                completed_at = NULL
            WHERE intent_id = ?1
            "#,
            params![
                intent_id,
                WriteIntentStatus::Failed.to_string(),
                error.trim().to_owned(),
            ],
        )?;
        Ok(())
    }
    pub fn mark_intent_complete(&self, intent_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            UPDATE write_intents
            SET status = ?2,
                completed_at = unixepoch(),
                error_message = NULL
            WHERE intent_id = ?1
            "#,
            params![intent_id, WriteIntentStatus::Complete.to_string()],
        )?;
        Ok(())
    }
    pub fn batch_complete_intents(
        &self,
        intent_ids: &[String],
    ) -> Result<BatchCompleteResult, StoreError> {
        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Deferred)?;

        let (completed, failed) = {
            let mut update_status = tx.prepare(
                r#"
                UPDATE write_intents
                SET status = ?2
                WHERE intent_id = ?1
                "#,
            )?;
            let mut mark_complete = tx.prepare(
                r#"
                UPDATE write_intents
                SET status = ?2,
                    completed_at = unixepoch(),
                    error_message = NULL
                WHERE intent_id = ?1
                "#,
            )?;
            let mut mark_failed = tx.prepare(
                r#"
                UPDATE write_intents
                SET status = ?2,
                    error_message = ?3,
                    completed_at = NULL
                WHERE intent_id = ?1
                "#,
            )?;

            let mut completed = 0usize;
            let mut failed = 0usize;

            for intent_id in intent_ids {
                if let Err(err) = update_status
                    .execute(params![intent_id, WriteIntentStatus::GraphDone.to_string()])
                {
                    let message = format!("graph_done update failed: {err:#}");
                    tracing::warn!(
                        intent_id = %intent_id,
                        error = %err,
                        "failed to update write intent status to graph_done in batch"
                    );
                    failed += 1;
                    if let Err(mark_err) = mark_failed.execute(params![
                        intent_id,
                        WriteIntentStatus::Failed.to_string(),
                        message.as_str(),
                    ]) {
                        tracing::error!(
                            intent_id = %intent_id,
                            error = %mark_err,
                            "failed to mark batched write intent as failed"
                        );
                    }
                    continue;
                }

                if let Err(err) = mark_complete
                    .execute(params![intent_id, WriteIntentStatus::Complete.to_string()])
                {
                    let message = format!("intent completion failed: {err:#}");
                    tracing::warn!(
                        intent_id = %intent_id,
                        error = %err,
                        "failed to mark batched write intent complete"
                    );
                    failed += 1;
                    if let Err(mark_err) = mark_failed.execute(params![
                        intent_id,
                        WriteIntentStatus::Failed.to_string(),
                        message.as_str(),
                    ]) {
                        tracing::error!(
                            intent_id = %intent_id,
                            error = %mark_err,
                            "failed to mark batched write intent as failed"
                        );
                    }
                    continue;
                }

                completed += 1;
            }

            (completed, failed)
        };

        tx.commit()?;
        Ok(BatchCompleteResult { completed, failed })
    }
    pub fn get_incomplete_intents(&self) -> Result<Vec<WriteIntent>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT intent_id, symbol_id, file_path, operation, status, payload_json, created_at, completed_at, error_message
            FROM write_intents
            WHERE status NOT IN ('complete', 'failed')
            ORDER BY created_at ASC, intent_id ASC
            "#,
        )?;
        let rows = stmt.query_map([], write_intent_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn get_failed_intents(&self) -> Result<Vec<WriteIntent>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT intent_id, symbol_id, file_path, operation, status, payload_json, created_at, completed_at, error_message
            FROM write_intents
            WHERE status = 'failed'
            ORDER BY created_at ASC, intent_id ASC
            "#,
        )?;
        let rows = stmt.query_map([], write_intent_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn get_intent(&self, intent_id: &str) -> Result<Option<WriteIntent>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT intent_id, symbol_id, file_path, operation, status, payload_json, created_at, completed_at, error_message
            FROM write_intents
            WHERE intent_id = ?1
            LIMIT 1
            "#,
        )?;
        stmt.query_row(params![intent_id], write_intent_from_row)
            .optional()
            .map_err(Into::into)
    }
    pub fn prune_completed_intents(&self, older_than_secs: i64) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute(
            r#"
            DELETE FROM write_intents
            WHERE status = 'complete'
              AND completed_at IS NOT NULL
              AND completed_at <= (unixepoch() - ?1)
            "#,
            params![older_than_secs.max(0)],
        )?;
        Ok(deleted)
    }
    pub fn count_intents_by_status(&self) -> Result<HashMap<String, usize>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT status, COUNT(*)
            FROM write_intents
            GROUP BY status
            ORDER BY status ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as usize,
            ))
        })?;
        let mut counts = HashMap::new();
        for row in rows {
            let (status, count) = row?;
            counts.insert(status, count);
        }
        Ok(counts)
    }
}
