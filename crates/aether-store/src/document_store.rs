use std::convert::TryFrom;

use aether_core::normalize_path;
use aether_document::{GenericRecord, GenericUnit};
use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use crate::{SqliteStore, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainStats {
    pub unit_count: u64,
    pub record_count: u64,
    pub source_count: u64,
    pub last_updated: Option<i64>,
}

impl SqliteStore {
    pub fn insert_document_unit(&self, unit: &GenericUnit) -> Result<(), StoreError> {
        let metadata_json = serde_json::to_string(&unit.metadata_json)?;
        let byte_range_start = u64_to_i64(unit.byte_range.0, "byte_range_start")?;
        let byte_range_end = u64_to_i64(unit.byte_range.1, "byte_range_end")?;
        let source_path = normalize_path(unit.source_path.as_str());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO document_units (
                unit_id, domain, unit_kind, display_name, content, source_path,
                byte_range_start, byte_range_end, parent_id, metadata_json, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, ?9, ?10, unixepoch(), unixepoch()
            )
            ON CONFLICT(unit_id) DO UPDATE SET
                domain = excluded.domain,
                unit_kind = excluded.unit_kind,
                display_name = excluded.display_name,
                content = excluded.content,
                source_path = excluded.source_path,
                byte_range_start = excluded.byte_range_start,
                byte_range_end = excluded.byte_range_end,
                parent_id = excluded.parent_id,
                metadata_json = excluded.metadata_json,
                updated_at = excluded.updated_at
            "#,
            params![
                unit.unit_id.as_str(),
                unit.domain.as_str(),
                unit.unit_kind.as_str(),
                unit.display_name.as_str(),
                unit.content.as_str(),
                source_path,
                byte_range_start,
                byte_range_end,
                unit.parent_id.as_deref(),
                metadata_json,
            ],
        )?;
        Ok(())
    }

    pub fn get_document_unit(&self, unit_id: &str) -> Result<Option<GenericUnit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                r#"
                SELECT
                    unit_id, domain, unit_kind, display_name, content, source_path,
                    byte_range_start, byte_range_end, parent_id, metadata_json
                FROM document_units
                WHERE unit_id = ?1
                LIMIT 1
                "#,
                params![unit_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, String>(9)?,
                    ))
                },
            )
            .optional()?;
        row.map(unit_from_row_tuple).transpose()
    }

    pub fn get_units_by_domain(&self, domain: &str) -> Result<Vec<GenericUnit>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                unit_id, domain, unit_kind, display_name, content, source_path,
                byte_range_start, byte_range_end, parent_id, metadata_json
            FROM document_units
            WHERE domain = ?1
            ORDER BY source_path ASC, byte_range_start ASC, unit_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![domain], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        let mut units = Vec::new();
        for row in rows {
            units.push(unit_from_row_tuple(row?)?);
        }
        Ok(units)
    }

    pub fn get_units_by_source(&self, source_path: &str) -> Result<Vec<GenericUnit>, StoreError> {
        let source_path = normalize_path(source_path);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                unit_id, domain, unit_kind, display_name, content, source_path,
                byte_range_start, byte_range_end, parent_id, metadata_json
            FROM document_units
            WHERE source_path = ?1
            ORDER BY byte_range_start ASC, unit_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![source_path], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        let mut units = Vec::new();
        for row in rows {
            units.push(unit_from_row_tuple(row?)?);
        }
        Ok(units)
    }

    pub fn delete_units_by_source(&self, source_path: &str) -> Result<usize, StoreError> {
        let source_path = normalize_path(source_path);
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            r#"
            DELETE FROM semantic_records
            WHERE unit_id IN (
                SELECT unit_id FROM document_units WHERE source_path = ?1
            )
            "#,
            params![source_path],
        )?;
        let deleted_units = tx.execute(
            "DELETE FROM document_units WHERE source_path = ?1",
            params![source_path],
        )?;
        tx.commit()?;
        Ok(deleted_units)
    }

    pub fn insert_semantic_record(&self, record: &GenericRecord) -> Result<(), StoreError> {
        if !record.record_json.is_object() {
            return Err(StoreError::Compatibility(
                "semantic_records.record_json must be a JSON object".to_owned(),
            ));
        }

        let record_json = serde_json::to_string(&record.record_json)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        // Remove stale records for this unit+schema before inserting the new version.
        // This prevents accumulation when content_hash changes (which changes record_id).
        tx.execute(
            "DELETE FROM semantic_records WHERE unit_id = ?1 AND schema_name = ?2 AND record_id != ?3",
            params![
                record.unit_id.as_str(),
                record.schema_name.as_str(),
                record.record_id.as_str(),
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO semantic_records (
                record_id, unit_id, domain, schema_name, schema_version, content_hash,
                record_json, embedding_text, created_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6,
                ?7, ?8, unixepoch(), unixepoch()
            )
            ON CONFLICT(record_id) DO UPDATE SET
                unit_id = excluded.unit_id,
                domain = excluded.domain,
                schema_name = excluded.schema_name,
                schema_version = excluded.schema_version,
                content_hash = excluded.content_hash,
                record_json = excluded.record_json,
                embedding_text = excluded.embedding_text,
                updated_at = excluded.updated_at
            "#,
            params![
                record.record_id.as_str(),
                record.unit_id.as_str(),
                record.domain.as_str(),
                record.schema_name.as_str(),
                record.schema_version.as_str(),
                record.content_hash.as_str(),
                record_json,
                record.embedding_text.as_str(),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_record_by_unit(&self, unit_id: &str) -> Result<Option<GenericRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                r#"
                SELECT
                    record_id, unit_id, domain, schema_name, schema_version,
                    content_hash, record_json, embedding_text
                FROM semantic_records
                WHERE unit_id = ?1
                ORDER BY updated_at DESC, record_id ASC
                LIMIT 1
                "#,
                params![unit_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;
        row.map(record_from_row_tuple).transpose()
    }

    pub fn get_records_by_domain(
        &self,
        domain: &str,
        limit: usize,
    ) -> Result<Vec<GenericRecord>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                record_id, unit_id, domain, schema_name, schema_version,
                content_hash, record_json, embedding_text
            FROM semantic_records
            WHERE domain = ?1
            ORDER BY updated_at DESC, record_id ASC
            LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![domain, to_sql_limit(limit)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(record_from_row_tuple(row?)?);
        }
        Ok(records)
    }

    pub fn search_records_lexical(
        &self,
        domain: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GenericRecord>, StoreError> {
        let query = query.trim();
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let pattern = format!("%{query}%");
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                record_id, unit_id, domain, schema_name, schema_version,
                content_hash, record_json, embedding_text
            FROM semantic_records
            WHERE domain = ?1
              AND (
                  LOWER(embedding_text) LIKE LOWER(?2)
                  OR LOWER(record_json) LIKE LOWER(?2)
              )
            ORDER BY updated_at DESC, record_id ASC
            LIMIT ?3
            "#,
        )?;
        let rows = stmt.query_map(params![domain, pattern, to_sql_limit(limit)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            records.push(record_from_row_tuple(row?)?);
        }
        Ok(records)
    }

    pub fn domain_stats(&self, domain: &str) -> Result<DomainStats, StoreError> {
        let conn = self.conn.lock().unwrap();
        let (unit_count, record_count, source_count, last_updated) = conn.query_row(
            r#"
            SELECT
                (SELECT COUNT(*) FROM document_units WHERE domain = ?1),
                (SELECT COUNT(*) FROM semantic_records WHERE domain = ?1),
                (SELECT COUNT(DISTINCT source_path) FROM document_units WHERE domain = ?1),
                (
                    SELECT MAX(updated_at) FROM (
                        SELECT updated_at FROM document_units WHERE domain = ?1
                        UNION ALL
                        SELECT updated_at FROM semantic_records WHERE domain = ?1
                    )
                )
            "#,
            params![domain],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )?;
        Ok(DomainStats {
            unit_count: non_negative_i64_to_u64(unit_count, "unit_count")?,
            record_count: non_negative_i64_to_u64(record_count, "record_count")?,
            source_count: non_negative_i64_to_u64(source_count, "source_count")?,
            last_updated,
        })
    }
}

type DocumentUnitRowTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    Option<String>,
    String,
);

type SemanticRecordRowTuple = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn unit_from_row_tuple(tuple: DocumentUnitRowTuple) -> Result<GenericUnit, StoreError> {
    let (
        unit_id,
        domain,
        unit_kind,
        display_name,
        content,
        source_path,
        byte_range_start,
        byte_range_end,
        parent_id,
        metadata_json,
    ) = tuple;
    let metadata_json: Value = serde_json::from_str(&metadata_json)?;
    Ok(GenericUnit {
        unit_id,
        domain,
        unit_kind,
        display_name,
        content,
        source_path,
        byte_range: (
            i64_to_u64(byte_range_start, "byte_range_start")?,
            i64_to_u64(byte_range_end, "byte_range_end")?,
        ),
        parent_id,
        metadata_json,
    })
}

fn record_from_row_tuple(tuple: SemanticRecordRowTuple) -> Result<GenericRecord, StoreError> {
    let (
        record_id,
        unit_id,
        domain,
        schema_name,
        schema_version,
        content_hash,
        record_json,
        embedding_text,
    ) = tuple;
    let record_json: Value = serde_json::from_str(&record_json)?;
    if !record_json.is_object() {
        return Err(StoreError::Compatibility(
            "semantic_records.record_json must be a JSON object".to_owned(),
        ));
    }
    Ok(GenericRecord {
        record_id,
        unit_id,
        domain,
        schema_name,
        schema_version,
        record_json,
        content_hash,
        embedding_text,
    })
}

fn to_sql_limit(limit: usize) -> Result<i64, StoreError> {
    i64::try_from(limit).map_err(|_| StoreError::Compatibility("limit exceeds i64".to_owned()))
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Compatibility(format!("{field} exceeds SQLite INTEGER range")))
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64, StoreError> {
    if value < 0 {
        return Err(StoreError::Compatibility(format!(
            "{field} cannot be negative in document store rows"
        )));
    }
    Ok(value as u64)
}

fn non_negative_i64_to_u64(value: i64, field: &str) -> Result<u64, StoreError> {
    i64_to_u64(value, field)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn insert_semantic_record_is_atomic_when_insert_fails() {
        let temp = tempdir().expect("tempdir");
        let workspace = temp.path();
        let store = SqliteStore::open(workspace).expect("open store");

        let unit = GenericUnit::new(
            "Intro",
            "AETHER documentation",
            "paragraph",
            "docs/intro.md",
            (0, 19),
            None,
            "docs",
        );
        store
            .insert_document_unit(&unit)
            .expect("insert document unit");

        let original = GenericRecord::new(
            unit.unit_id.clone(),
            "docs",
            "entity",
            "v1",
            json!({"title":"AETHER","state":"original"}),
            "AETHER original",
        )
        .expect("build original record");
        store
            .insert_semantic_record(&original)
            .expect("insert original semantic record");

        let sqlite_path = workspace.join(".aether").join("meta.sqlite");
        let conn = rusqlite::Connection::open(sqlite_path).expect("open sqlite");
        conn.execute_batch(
            r#"
            CREATE TRIGGER fail_semantic_insert
            BEFORE INSERT ON semantic_records
            BEGIN
                SELECT RAISE(ABORT, 'forced semantic insert failure');
            END;
            "#,
        )
        .expect("install failing trigger");
        drop(conn);

        let replacement = GenericRecord::new(
            unit.unit_id.clone(),
            "docs",
            "entity",
            "v1",
            json!({"title":"AETHER","state":"replacement"}),
            "AETHER replacement",
        )
        .expect("build replacement record");
        let err = store
            .insert_semantic_record(&replacement)
            .expect_err("replacement insert should fail due to trigger");
        match err {
            StoreError::Sqlite(inner) => {
                let message = inner.to_string();
                assert!(
                    message.contains("forced semantic insert failure"),
                    "unexpected sqlite error: {message}"
                );
            }
            other => panic!("expected sqlite error, got {other}"),
        }

        let retained = store
            .get_record_by_unit(unit.unit_id.as_str())
            .expect("query retained record")
            .expect("record should remain after failed upsert");
        assert_eq!(retained.record_id, original.record_id);
        assert_eq!(retained.record_json, original.record_json);
        assert_eq!(retained.embedding_text, original.embedding_text);
    }
}
