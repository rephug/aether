use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectEntityRefRecord {
    pub kind: String,
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNoteRecord {
    pub note_id: String,
    pub content: String,
    pub content_hash: String,
    pub source_type: String,
    pub source_agent: Option<String>,
    pub tags: Vec<String>,
    pub entity_refs: Vec<ProjectEntityRefRecord>,
    pub file_refs: Vec<String>,
    pub symbol_refs: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
    pub is_archived: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNoteEmbeddingRecord {
    pub note_id: String,
    pub provider: String,
    pub model: String,
    pub embedding: Vec<f32>,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectNoteSemanticSearchResult {
    pub note_id: String,
    pub semantic_score: f32,
}
type ProjectNoteRowTuple = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
    i64,
);
fn project_note_tuple_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectNoteRowTuple> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
    ))
}
fn project_note_from_tuple(tuple: ProjectNoteRowTuple) -> Result<ProjectNoteRecord, StoreError> {
    let (
        note_id,
        content,
        content_hash,
        source_type,
        source_agent,
        tags_json,
        entity_refs_json,
        file_refs_json,
        symbol_refs_json,
        created_at,
        updated_at,
        access_count,
        last_accessed_at,
        is_archived,
    ) = tuple;

    Ok(ProjectNoteRecord {
        note_id,
        content,
        content_hash,
        source_type,
        source_agent,
        tags: parse_string_array_json(&tags_json)?,
        entity_refs: parse_project_entity_refs_json(&entity_refs_json)?,
        file_refs: parse_string_array_json(&file_refs_json)?,
        symbol_refs: parse_string_array_json(&symbol_refs_json)?,
        created_at,
        updated_at,
        access_count,
        last_accessed_at,
        is_archived: is_archived != 0,
    })
}
fn parse_string_array_json(raw: &str) -> Result<Vec<String>, StoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    Ok(json_from_str::<Vec<String>>(trimmed)?)
}
fn parse_project_entity_refs_json(raw: &str) -> Result<Vec<ProjectEntityRefRecord>, StoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let value = serde_json::from_str::<serde_json::Value>(trimmed)?;
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };

    let mut refs = Vec::new();
    for item in items {
        let kind = item.get("kind").and_then(serde_json::Value::as_str);
        let id = item.get("id").and_then(serde_json::Value::as_str);
        let (Some(kind), Some(id)) = (kind, id) else {
            continue;
        };

        let kind = kind.trim();
        let id = id.trim();
        if kind.is_empty() || id.is_empty() {
            continue;
        }

        refs.push(ProjectEntityRefRecord {
            kind: kind.to_owned(),
            id: id.to_owned(),
        });
    }

    Ok(refs)
}
fn project_entity_refs_to_json(
    entity_refs: &[ProjectEntityRefRecord],
) -> Result<String, StoreError> {
    let values = entity_refs
        .iter()
        .map(|entity| {
            serde_json::json!({
                "kind": entity.kind,
                "id": entity.id,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string(&values)?)
}

impl SqliteStore {
    pub(crate) fn store_upsert_project_note(
        &self,
        record: ProjectNoteRecord,
    ) -> Result<(), StoreError> {
        let tags_json = serde_json::to_string(&record.tags)?;
        let entity_refs_json = project_entity_refs_to_json(&record.entity_refs)?;
        let file_refs_json = serde_json::to_string(&record.file_refs)?;
        let symbol_refs_json = serde_json::to_string(&record.symbol_refs)?;
        let archived = if record.is_archived { 1 } else { 0 };

        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO project_notes (
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            ON CONFLICT(note_id) DO UPDATE SET
                content = excluded.content,
                content_hash = excluded.content_hash,
                source_type = excluded.source_type,
                source_agent = excluded.source_agent,
                tags = excluded.tags,
                entity_refs = excluded.entity_refs,
                file_refs = excluded.file_refs,
                symbol_refs = excluded.symbol_refs,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                access_count = excluded.access_count,
                last_accessed_at = excluded.last_accessed_at,
                is_archived = excluded.is_archived
            "#,
            params![
                record.note_id,
                record.content,
                record.content_hash,
                record.source_type,
                record.source_agent,
                tags_json,
                entity_refs_json,
                file_refs_json,
                symbol_refs_json,
                record.created_at,
                record.updated_at,
                record.access_count,
                record.last_accessed_at,
                archived,
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_find_project_note_by_content_hash(
        &self,
        content_hash: &str,
        include_archived: bool,
    ) -> Result<Option<ProjectNoteRecord>, StoreError> {
        let content_hash = content_hash.trim();
        if content_hash.is_empty() {
            return Ok(None);
        }

        let sql = if include_archived {
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE content_hash = ?1
            ORDER BY updated_at DESC, note_id ASC
            LIMIT 1
            "#
        } else {
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE content_hash = ?1
              AND is_archived = 0
            ORDER BY updated_at DESC, note_id ASC
            LIMIT 1
            "#
        };

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let row = stmt
            .query_row(params![content_hash], |row| {
                project_note_tuple_from_row(row)
            })
            .optional()?;

        row.map(project_note_from_tuple).transpose()
    }
    pub(crate) fn store_get_project_note(
        &self,
        note_id: &str,
    ) -> Result<Option<ProjectNoteRecord>, StoreError> {
        let note_id = note_id.trim();
        if note_id.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE note_id = ?1
            LIMIT 1
            "#,
        )?;

        let row = stmt
            .query_row(params![note_id], project_note_tuple_from_row)
            .optional()?;

        row.map(project_note_from_tuple).transpose()
    }
    pub(crate) fn store_list_project_notes(
        &self,
        limit: u32,
        since_epoch_ms: Option<i64>,
        include_archived: bool,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        let mut sql = String::from(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE 1 = 1
            "#,
        );
        let mut params_vec: Vec<SqlValue> = Vec::new();

        if !include_archived {
            sql.push_str(" AND is_archived = 0");
        }
        if let Some(since) = since_epoch_ms {
            sql.push_str(" AND updated_at >= ?");
            params_vec.push(SqlValue::Integer(since.max(0)));
        }

        sql.push_str(" ORDER BY updated_at DESC, note_id ASC LIMIT ?");
        params_vec.push(SqlValue::Integer(limit.clamp(1, 100) as i64));

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), project_note_tuple_from_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(project_note_from_tuple(row?)?);
        }

        Ok(records)
    }
    pub(crate) fn store_list_project_notes_for_file_ref(
        &self,
        file_path: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE is_archived = 0
              AND EXISTS (
                  SELECT 1
                  FROM json_each(project_notes.file_refs)
                  WHERE json_each.value = ?1
              )
            ORDER BY updated_at DESC, note_id ASC
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(
            params![file_path, limit.clamp(1, 100)],
            project_note_tuple_from_row,
        )?;
        let mut records = Vec::new();
        for row in rows {
            records.push(project_note_from_tuple(row?)?);
        }

        Ok(records)
    }
    pub(crate) fn store_search_project_notes_lexical(
        &self,
        query: &str,
        limit: u32,
        include_archived: bool,
        tags_filter: &[String],
    ) -> Result<Vec<ProjectNoteRecord>, StoreError> {
        let query = query.trim();
        let mut sql = String::from(
            r#"
            SELECT
                note_id, content, content_hash, source_type, source_agent,
                tags, entity_refs, file_refs, symbol_refs,
                created_at, updated_at, access_count, last_accessed_at, is_archived
            FROM project_notes
            WHERE 1 = 1
            "#,
        );
        let mut params_vec: Vec<SqlValue> = Vec::new();

        if !include_archived {
            sql.push_str(" AND is_archived = 0");
        }
        if !query.is_empty() {
            let terms = project_note_lexical_terms(query);
            if !terms.is_empty() {
                sql.push_str(" AND (");
                for (index, term) in terms.iter().enumerate() {
                    if index > 0 {
                        sql.push_str(" OR ");
                    }
                    sql.push_str("(LOWER(content) LIKE ? OR LOWER(tags) LIKE ?)");

                    let pattern = format!("%{term}%");
                    params_vec.push(SqlValue::Text(pattern.clone()));
                    params_vec.push(SqlValue::Text(pattern));
                }
                sql.push(')');
            }
        }

        for tag in tags_filter
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
        {
            sql.push_str(
                " AND EXISTS (
                    SELECT 1 FROM json_each(tags) AS je
                    WHERE LOWER(je.value) = LOWER(?)
                )",
            );
            params_vec.push(SqlValue::Text(tag.to_owned()));
        }

        sql.push_str(" ORDER BY updated_at DESC, note_id ASC LIMIT ?");
        params_vec.push(SqlValue::Integer(limit.clamp(1, 100) as i64));

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), project_note_tuple_from_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(project_note_from_tuple(row?)?);
        }

        Ok(records)
    }
    pub(crate) fn store_increment_project_note_access(
        &self,
        note_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        if note_ids.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        if conn
            .is_readonly(rusqlite::DatabaseName::Main)
            .unwrap_or(false)
        {
            return Ok(());
        }
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                r#"
                UPDATE project_notes
                SET access_count = access_count + 1,
                    last_accessed_at = ?2
                WHERE note_id = ?1
                "#,
            )?;

            for note_id in note_ids {
                let trimmed = note_id.trim();
                if trimmed.is_empty() {
                    continue;
                }
                stmt.execute(params![trimmed, accessed_at.max(0)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn store_upsert_project_note_embedding(
        &self,
        record: ProjectNoteEmbeddingRecord,
    ) -> Result<(), StoreError> {
        if record.embedding.is_empty() {
            return Ok(());
        }

        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO project_notes_embeddings (
                note_id, provider, model, embedding_dim, embedding_json, content, created_at, updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(note_id) DO UPDATE SET
                provider = excluded.provider,
                model = excluded.model,
                embedding_dim = excluded.embedding_dim,
                embedding_json = excluded.embedding_json,
                content = excluded.content,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            "#,
            params![
                record.note_id,
                record.provider,
                record.model,
                record.embedding.len() as i64,
                serde_json::to_string(&record.embedding)?,
                record.content,
                record.created_at.max(0),
                record.updated_at.max(0),
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_delete_project_note_embedding(
        &self,
        note_id: &str,
    ) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM project_notes_embeddings WHERE note_id = ?1",
            params![note_id],
        )?;
        Ok(())
    }
    pub(crate) fn store_search_project_notes_semantic(
        &self,
        query_embedding: &[f32],
        provider: &str,
        model: &str,
        limit: u32,
    ) -> Result<Vec<ProjectNoteSemanticSearchResult>, StoreError> {
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
                SELECT note_id, embedding_json
                FROM project_notes_embeddings
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
                let (note_id, embedding_json) = row?;
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
                scored.push(ProjectNoteSemanticSearchResult {
                    note_id,
                    semantic_score: score,
                });
            }
        }

        scored.sort_by(|left, right| {
            right
                .semantic_score
                .partial_cmp(&left.semantic_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.note_id.cmp(&right.note_id))
        });
        scored.truncate(capped_limit);
        Ok(scored)
    }
}
