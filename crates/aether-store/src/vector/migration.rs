use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;

use super::lancedb::{LanceVectorStore, table_name_for};
use super::{ProjectNoteVectorRecord, VectorRecord};

impl LanceVectorStore {
    pub(super) async fn migrate_from_sqlite_if_needed(&self) -> Result<(), StoreError> {
        if self.marker_path().exists() {
            return Ok(());
        }

        let records = load_sqlite_embedding_rows(&self.sqlite_path())?;
        if records.is_empty() {
            fs::write(self.marker_path(), b"empty")?;
            return Ok(());
        }

        tracing::info!(
            count = records.len(),
            "migrating SQLite embeddings into LanceDB"
        );
        let mut records_by_table = BTreeMap::<String, Vec<VectorRecord>>::new();
        for record in records {
            if record.embedding.is_empty() {
                continue;
            }

            let table_name = table_name_for(
                record.provider.as_str(),
                record.model.as_str(),
                record.embedding.len() as i32,
            );
            records_by_table.entry(table_name).or_default().push(record);
        }
        if records_by_table.is_empty() {
            fs::write(self.marker_path(), b"empty")?;
            return Ok(());
        }

        let connection = self.connect().await?;
        for (table_name, table_records) in records_by_table {
            self.upsert_embedding_batch_with_connection(
                &connection,
                table_name.as_str(),
                table_records.as_slice(),
            )
            .await?;
        }

        fs::write(self.marker_path(), b"done")?;
        tracing::info!("completed LanceDB vector migration");
        Ok(())
    }

    pub(super) async fn migrate_project_notes_from_sqlite_if_needed(
        &self,
    ) -> Result<(), StoreError> {
        if self.project_notes_marker_path().exists() {
            return Ok(());
        }

        let records = load_sqlite_project_note_embedding_rows(&self.sqlite_path())?;
        if records.is_empty() {
            fs::write(self.project_notes_marker_path(), b"empty")?;
            return Ok(());
        }

        tracing::info!(
            count = records.len(),
            "migrating SQLite project note embeddings into LanceDB"
        );
        let connection = self.connect().await?;
        for record in records {
            self.upsert_project_note_embedding_with_connection(&connection, &record)
                .await?;
        }

        fs::write(self.project_notes_marker_path(), b"done")?;
        tracing::info!("completed LanceDB project note vector migration");
        Ok(())
    }
}

pub(super) fn load_sqlite_embedding_rows(
    sqlite_path: &Path,
) -> Result<Vec<VectorRecord>, StoreError> {
    if !sqlite_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open(sqlite_path)?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sir_embeddings' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT symbol_id, sir_hash, provider, model, embedding_json, updated_at
        FROM sir_embeddings
        "#,
    )?;
    let rows = stmt.query_map(params![], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;

    let mut records = Vec::new();
    for row in rows {
        let (symbol_id, sir_hash, provider, model, embedding_json, updated_at) = row?;
        let embedding = serde_json::from_str::<Vec<f32>>(&embedding_json)?;
        if embedding.is_empty() {
            continue;
        }
        records.push(VectorRecord {
            symbol_id,
            sir_hash,
            provider,
            model,
            embedding,
            updated_at,
        });
    }

    Ok(records)
}

pub(super) fn load_sqlite_project_note_embedding_rows(
    sqlite_path: &Path,
) -> Result<Vec<ProjectNoteVectorRecord>, StoreError> {
    if !sqlite_path.exists() {
        return Ok(Vec::new());
    }

    let conn = Connection::open(sqlite_path)?;
    let exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='project_notes_embeddings' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if !exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT note_id, provider, model, embedding_json, content, created_at, updated_at
        FROM project_notes_embeddings
        "#,
    )?;
    let rows = stmt.query_map(params![], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?;

    let mut records = Vec::new();
    for row in rows {
        let (note_id, provider, model, embedding_json, content, created_at, updated_at) = row?;
        let embedding = serde_json::from_str::<Vec<f32>>(&embedding_json)?;
        if embedding.is_empty() {
            continue;
        }
        records.push(ProjectNoteVectorRecord {
            note_id,
            provider,
            model,
            embedding,
            content,
            created_at,
            updated_at,
        });
    }

    Ok(records)
}
