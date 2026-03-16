use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContextHistoryRecord {
    pub task_description: String,
    pub branch_name: Option<String>,
    pub resolved_symbol_ids: String,
    pub resolved_file_paths: String,
    pub total_symbols: i64,
    pub budget_used: i64,
    pub budget_max: i64,
    pub created_at: i64,
}

impl SqliteStore {
    pub fn insert_task_context_history(
        &self,
        record: &TaskContextHistoryRecord,
    ) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO task_context_history (
                task_description,
                branch_name,
                resolved_symbol_ids,
                resolved_file_paths,
                total_symbols,
                budget_used,
                budget_max,
                created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                record.task_description,
                record.branch_name,
                record.resolved_symbol_ids,
                record.resolved_file_paths,
                record.total_symbols.max(0),
                record.budget_used.max(0),
                record.budget_max.max(0),
                record.created_at.max(0),
            ],
        )?;
        Ok(())
    }

    pub fn list_recent_task_history(
        &self,
        limit: usize,
    ) -> Result<Vec<TaskContextHistoryRecord>, StoreError> {
        let capped_limit = limit.clamp(1, 1000) as i64;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT
                task_description,
                branch_name,
                resolved_symbol_ids,
                resolved_file_paths,
                total_symbols,
                budget_used,
                budget_max,
                created_at
            FROM task_context_history
            ORDER BY created_at DESC, id DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![capped_limit], |row| {
            Ok(TaskContextHistoryRecord {
                task_description: row.get(0)?,
                branch_name: row.get(1)?,
                resolved_symbol_ids: row.get(2)?,
                resolved_file_paths: row.get(3)?,
                total_symbols: row.get::<_, i64>(4)?.max(0),
                budget_used: row.get::<_, i64>(5)?.max(0),
                budget_max: row.get::<_, i64>(6)?.max(0),
                created_at: row.get::<_, i64>(7)?.max(0),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}
