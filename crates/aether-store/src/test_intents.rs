use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestIntentRecord {
    pub intent_id: String,
    pub file_path: String,
    pub test_name: String,
    pub intent_text: String,
    pub group_label: Option<String>,
    pub language: String,
    pub symbol_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
type TestIntentRowTuple = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    i64,
    i64,
);
fn test_intent_tuple_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TestIntentRowTuple> {
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
    ))
}
fn test_intent_from_tuple(tuple: TestIntentRowTuple) -> Result<TestIntentRecord, StoreError> {
    let (
        intent_id,
        file_path,
        test_name,
        intent_text,
        group_label,
        language,
        symbol_id,
        created_at,
        updated_at,
    ) = tuple;

    Ok(TestIntentRecord {
        intent_id,
        file_path,
        test_name,
        intent_text,
        group_label,
        language,
        symbol_id,
        created_at,
        updated_at,
    })
}
fn build_test_intent_id(file_path: &str, test_name: &str, intent_text: &str) -> String {
    let material = format!(
        "{}\n{}\n{}",
        normalize_path(file_path.trim()),
        test_name.trim(),
        intent_text.trim(),
    );
    content_hash(material.as_str())
}

impl SqliteStore {
    pub(crate) fn store_replace_test_intents_for_file(
        &self,
        file_path: &str,
        intents: &[TestIntentRecord],
    ) -> Result<(), StoreError> {
        let file_path = normalize_path(file_path.trim());
        if file_path.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM test_intents WHERE file_path = ?1",
            params![file_path],
        )?;

        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO test_intents (
                    intent_id, file_path, test_name, intent_text, group_label,
                    language, symbol_id, created_at, updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
            )?;

            for intent in intents {
                let test_name = intent.test_name.trim();
                let intent_text = intent.intent_text.trim();
                if test_name.is_empty() || intent_text.is_empty() {
                    continue;
                }

                let record_id = if intent.intent_id.trim().is_empty() {
                    build_test_intent_id(file_path.as_str(), test_name, intent_text)
                } else {
                    intent.intent_id.trim().to_owned()
                };

                stmt.execute(params![
                    record_id,
                    file_path,
                    test_name,
                    intent_text,
                    intent.group_label.as_deref(),
                    intent.language.trim().to_ascii_lowercase(),
                    intent.symbol_id.as_deref().map(str::trim),
                    intent.created_at.max(0),
                    intent.updated_at.max(0),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn store_list_test_intents_for_file(
        &self,
        file_path: &str,
    ) -> Result<Vec<TestIntentRecord>, StoreError> {
        let file_path = normalize_path(file_path.trim());
        if file_path.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                intent_id, file_path, test_name, intent_text, group_label,
                language, symbol_id, created_at, updated_at
            FROM test_intents
            WHERE file_path = ?1
            ORDER BY test_name ASC, intent_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![file_path], test_intent_tuple_from_row)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(test_intent_from_tuple(row?)?);
        }
        Ok(records)
    }
    pub(crate) fn store_list_test_intents_for_symbol(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<TestIntentRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                intent_id, file_path, test_name, intent_text, group_label,
                language, symbol_id, created_at, updated_at
            FROM test_intents
            WHERE symbol_id = ?1
            ORDER BY file_path ASC, test_name ASC, intent_id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id], test_intent_tuple_from_row)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(test_intent_from_tuple(row?)?);
        }
        Ok(records)
    }
    pub(crate) fn store_search_test_intents_lexical(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<TestIntentRecord>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let terms = project_note_lexical_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            r#"
            SELECT
                intent_id, file_path, test_name, intent_text, group_label,
                language, symbol_id, created_at, updated_at
            FROM test_intents
            WHERE
            "#,
        );
        let mut params_vec = Vec::<SqlValue>::new();

        for (index, term) in terms.iter().enumerate() {
            if index > 0 {
                sql.push_str(" OR ");
            }
            sql.push_str(
                "(LOWER(test_name) LIKE ? OR LOWER(intent_text) LIKE ? OR LOWER(COALESCE(group_label, '')) LIKE ? OR LOWER(file_path) LIKE ?)",
            );

            let pattern = format!("%{term}%");
            params_vec.push(SqlValue::Text(pattern.clone()));
            params_vec.push(SqlValue::Text(pattern.clone()));
            params_vec.push(SqlValue::Text(pattern.clone()));
            params_vec.push(SqlValue::Text(pattern));
        }

        sql.push_str(" ORDER BY updated_at DESC, intent_id ASC LIMIT ?");
        params_vec.push(SqlValue::Integer(limit.clamp(1, 100) as i64));

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql.as_str())?;
        let rows = stmt.query_map(params_from_iter(params_vec), test_intent_tuple_from_row)?;

        let mut records = Vec::new();
        for row in rows {
            records.push(test_intent_from_tuple(row?)?);
        }
        Ok(records)
    }
}
