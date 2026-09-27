use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolEmbeddingRecord {
    pub symbol_id: String,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
    pub updated_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEmbeddingMetaRecord {
    pub symbol_id: String,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub embedding_dim: i64,
    pub updated_at: i64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticSearchResult {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub semantic_score: f32,
}

impl SqliteStore {
    pub fn list_all_embedding_symbol_ids(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT DISTINCT symbol_id
            FROM sir_embeddings
            ORDER BY symbol_id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn list_symbol_embeddings_for_ids(
        &self,
        provider: &str,
        model: &str,
        symbol_ids: &[String],
    ) -> Result<Vec<SymbolEmbeddingRecord>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() || symbol_ids.is_empty() {
            return Ok(Vec::new());
        }

        let normalized = symbol_ids
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
                SELECT symbol_id, sir_hash, provider, model, embedding_json, updated_at
                FROM sir_embeddings
                WHERE provider = ?1
                  AND model = ?2
                  AND symbol_id IN ({placeholders})
                ORDER BY symbol_id ASC
                "#
            );

            let mut params_vec: Vec<SqlValue> = vec![
                SqlValue::Text(provider.to_owned()),
                SqlValue::Text(model.to_owned()),
            ];
            params_vec.extend(chunk.iter().cloned().map(SqlValue::Text));

            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(params_from_iter(params_vec), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?;

            for row in rows {
                let (symbol_id, sir_hash, provider, model, embedding_json, updated_at) = row?;
                let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
                if embedding.is_empty() {
                    continue;
                }
                records.push(SymbolEmbeddingRecord {
                    symbol_id,
                    sir_hash,
                    provider,
                    model,
                    embedding,
                    updated_at,
                });
            }
        }
        records.sort_by(|left, right| left.symbol_id.cmp(&right.symbol_id));

        Ok(records)
    }
    pub(crate) fn store_upsert_symbol_embedding(
        &self,
        record: SymbolEmbeddingRecord,
    ) -> Result<(), StoreError> {
        let embedding_dim = record.embedding.len() as i64;
        let embedding_json = serde_json::to_string(&record.embedding)?;

        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO sir_embeddings (
                symbol_id, sir_hash, provider, model, embedding_dim, embedding_json, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(symbol_id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                provider = excluded.provider,
                model = excluded.model,
                embedding_dim = excluded.embedding_dim,
                embedding_json = excluded.embedding_json,
                updated_at = excluded.updated_at
            "#,
            params![
                record.symbol_id,
                record.sir_hash,
                record.provider,
                record.model,
                embedding_dim,
                embedding_json,
                record.updated_at,
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_get_symbol_embedding_meta(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolEmbeddingMetaRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT symbol_id, sir_hash, provider, model, embedding_dim, updated_at
            FROM sir_embeddings
            WHERE symbol_id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SymbolEmbeddingMetaRecord {
                    symbol_id: row.get(0)?,
                    sir_hash: row.get(1)?,
                    provider: row.get(2)?,
                    model: row.get(3)?,
                    embedding_dim: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .optional()?;

        Ok(record)
    }
    pub(crate) fn store_delete_symbol_embedding(&self, symbol_id: &str) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM sir_embeddings WHERE symbol_id = ?1",
            params![symbol_id],
        )?;
        Ok(())
    }
    /// Store the embedding only while the symbol's stored vector still carries
    /// `expected_sir_hash` (`None`: no row yet); the check and the write share one
    /// immediate transaction. Returns whether the row was written.
    pub fn upsert_symbol_embedding_if_sir_hash(
        &self,
        record: SymbolEmbeddingRecord,
        expected_sir_hash: Option<&str>,
    ) -> Result<bool, StoreError> {
        use rusqlite::OptionalExtension;
        let embedding_dim = record.embedding.len() as i64;
        let embedding_json = serde_json::to_string(&record.embedding)?;
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tx =
            rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT sir_hash FROM sir_embeddings WHERE symbol_id = ?1",
                params![record.symbol_id],
                |row| row.get(0),
            )
            .optional()?;
        if current.as_deref() != expected_sir_hash {
            return Ok(false);
        }
        tx.execute(
            r#"
            INSERT INTO sir_embeddings (
                symbol_id, sir_hash, provider, model, embedding_dim, embedding_json, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(symbol_id) DO UPDATE SET
                sir_hash = excluded.sir_hash,
                provider = excluded.provider,
                model = excluded.model,
                embedding_dim = excluded.embedding_dim,
                embedding_json = excluded.embedding_json,
                updated_at = excluded.updated_at
            "#,
            params![
                record.symbol_id,
                record.sir_hash,
                record.provider,
                record.model,
                embedding_dim,
                embedding_json,
                record.updated_at,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
    /// Delete the symbol's embedding only while it still carries `sir_hash` (one
    /// statement, so no newer vector can slip in between a check and the delete).
    pub fn delete_symbol_embedding_if_sir_hash(
        &self,
        symbol_id: &str,
        sir_hash: &str,
    ) -> Result<(), StoreError> {
        // A panic elsewhere while holding the connection poisons the mutex, but the
        // connection itself is still usable: recover it rather than panic in turn.
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "DELETE FROM sir_embeddings WHERE symbol_id = ?1 AND sir_hash = ?2",
            params![symbol_id, sir_hash],
        )?;
        Ok(())
    }
    pub(crate) fn store_search_symbols_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<SemanticSearchResult>, StoreError> {
        let provider = provider.trim();
        let model = model.trim();
        if query_embedding.is_empty() || provider.is_empty() || model.is_empty() {
            return Ok(Vec::new());
        }

        let query_norm_sq = query_embedding
            .iter()
            .map(|value| value * value)
            .fold(0.0f32, |acc, value| acc + value);
        if query_norm_sq <= f32::EPSILON {
            return Ok(Vec::new());
        }
        let query_norm = query_norm_sq.sqrt();
        let capped_limit = limit.clamp(1, 100) as usize;

        let mut scored = Vec::new();
        {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                r#"
                SELECT symbol_id, embedding_json
                FROM sir_embeddings
                WHERE provider = ?1
                  AND model = ?2
                  AND embedding_dim = ?3
                "#,
            )?;
            let rows = stmt.query_map(
                params![provider, model, query_embedding.len() as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;

            for row in rows {
                let (symbol_id, embedding_json) = row?;
                let embedding = json_from_str::<Vec<f32>>(&embedding_json)?;
                if embedding.len() != query_embedding.len() {
                    continue;
                }

                let dot = embedding
                    .iter()
                    .zip(query_embedding.iter())
                    .map(|(left, right)| left * right)
                    .fold(0.0f32, |acc, value| acc + value);
                let embedding_norm_sq = embedding
                    .iter()
                    .map(|value| value * value)
                    .fold(0.0f32, |acc, value| acc + value);
                if embedding_norm_sq <= f32::EPSILON {
                    continue;
                }

                let score = dot / (embedding_norm_sq.sqrt() * query_norm);
                scored.push((symbol_id, score));
            }
        }

        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut results = Vec::new();
        for (symbol_id, score) in scored.into_iter().take(capped_limit) {
            let Some(symbol) = self.get_symbol_search_result(&symbol_id)? else {
                continue;
            };

            results.push(SemanticSearchResult {
                symbol_id: symbol.symbol_id,
                qualified_name: symbol.qualified_name,
                file_path: symbol.file_path,
                language: symbol.language,
                kind: symbol.kind,
                semantic_score: score,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn record(symbol_id: &str, sir_hash: &str) -> SymbolEmbeddingRecord {
        SymbolEmbeddingRecord {
            symbol_id: symbol_id.to_owned(),
            sir_hash: sir_hash.to_owned(),
            provider: "p".to_owned(),
            model: "m".to_owned(),
            embedding: vec![1.0, 0.0],
            updated_at: 1,
        }
    }

    #[test]
    fn conditional_embedding_upsert_only_replaces_the_expected_hash() {
        let temp = tempdir().expect("tempdir");
        let store = SqliteStore::open(temp.path()).expect("open store");

        // Nothing stored: a write expecting no row lands, one expecting a row does not.
        assert!(
            !store
                .upsert_symbol_embedding_if_sir_hash(record("s", "h1"), Some("h0"))
                .expect("cas")
        );
        assert!(
            store
                .upsert_symbol_embedding_if_sir_hash(record("s", "h1"), None)
                .expect("cas")
        );
        // A row exists: only a writer that saw h1 may replace it.
        assert!(
            !store
                .upsert_symbol_embedding_if_sir_hash(record("s", "h2"), None)
                .expect("cas")
        );
        assert!(
            !store
                .upsert_symbol_embedding_if_sir_hash(record("s", "h2"), Some("stale"))
                .expect("cas")
        );
        assert!(
            store
                .upsert_symbol_embedding_if_sir_hash(record("s", "h2"), Some("h1"))
                .expect("cas")
        );
        let meta = store
            .get_symbol_embedding_meta("s")
            .expect("meta")
            .expect("row");
        assert_eq!(meta.sir_hash, "h2");

        // The hash-conditional delete is the mirror image.
        store
            .delete_symbol_embedding_if_sir_hash("s", "h1")
            .expect("delete");
        assert!(
            store
                .get_symbol_embedding_meta("s")
                .expect("meta")
                .is_some()
        );
        store
            .delete_symbol_embedding_if_sir_hash("s", "h2")
            .expect("delete");
        assert!(
            store
                .get_symbol_embedding_meta("s")
                .expect("meta")
                .is_none()
        );
    }
}
