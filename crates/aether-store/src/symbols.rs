use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolRecord {
    pub id: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub qualified_name: String,
    pub signature_fingerprint: String,
    pub last_seen_at: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolMetadata {
    pub symbol_id: String,
    pub kind: String,
    pub file_path: String,
    pub is_public: bool,
    pub line_count: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSearchResult {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub language: String,
    pub kind: String,
    pub access_count: i64,
    pub last_accessed_at: Option<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SymbolAccessState {
    pub(crate) access_count: i64,
    pub(crate) last_accessed_at: Option<i64>,
}
fn infer_symbol_is_public(path: &Path, qualified_name: &str) -> bool {
    let Ok(source) = fs::read_to_string(path) else {
        return false;
    };
    let leaf = qualified_name
        .rsplit("::")
        .next()
        .or_else(|| qualified_name.rsplit('.').next())
        .unwrap_or(qualified_name)
        .trim();
    if leaf.is_empty() {
        return false;
    }

    source.lines().any(|line| {
        let normalized = line.trim();
        (normalized.starts_with("pub ") || normalized.starts_with("export "))
            && normalized.contains(leaf)
    })
}
pub(crate) fn load_symbol_access_state(
    tx: &Transaction<'_>,
    symbol_id: &str,
) -> Result<Option<SymbolAccessState>, StoreError> {
    tx.query_row(
        r#"
        SELECT access_count, last_accessed_at
        FROM symbols
        WHERE id = ?1
        "#,
        params![symbol_id],
        |row| {
            Ok(SymbolAccessState {
                access_count: row.get::<_, Option<i64>>(0)?.unwrap_or(0).max(0),
                last_accessed_at: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
pub(crate) fn merge_symbol_access_state(
    current: SymbolAccessState,
    stale: SymbolAccessState,
) -> SymbolAccessState {
    SymbolAccessState {
        access_count: current
            .access_count
            .max(0)
            .saturating_add(stale.access_count.max(0)),
        last_accessed_at: match (current.last_accessed_at, stale.last_accessed_at) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        },
    }
}
pub(crate) fn update_symbol_access_state(
    tx: &Transaction<'_>,
    symbol_id: &str,
    access: &SymbolAccessState,
) -> Result<(), StoreError> {
    tx.execute(
        r#"
        UPDATE symbols
        SET access_count = ?2,
            last_accessed_at = ?3
        WHERE id = ?1
        "#,
        params![
            symbol_id,
            access.access_count.max(0),
            access.last_accessed_at
        ],
    )?;
    Ok(())
}

impl SqliteStore {
    pub fn get_symbol_search_result(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolSearchResult>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, qualified_name, file_path, language, kind, access_count, last_accessed_at
            FROM symbols
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SymbolSearchResult {
                    symbol_id: row.get(0)?,
                    qualified_name: row.get(1)?,
                    file_path: row.get(2)?,
                    language: row.get(3)?,
                    kind: row.get(4)?,
                    access_count: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0),
                    last_accessed_at: row.get(6)?,
                })
            })
            .optional()?;

        Ok(record)
    }
    pub fn get_symbol_search_results_batch(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, SymbolSearchResult>, StoreError> {
        let normalized = ids
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if normalized.is_empty() {
            return Ok(HashMap::new());
        }

        let mut records = HashMap::new();
        let conn = self.conn.lock().unwrap();
        for chunk in normalized.chunks(SQLITE_PARAM_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                r#"
                SELECT id, qualified_name, file_path, language, kind, access_count, last_accessed_at
                FROM symbols
                WHERE id IN ({placeholders})
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
                Ok(SymbolSearchResult {
                    symbol_id: row.get(0)?,
                    qualified_name: row.get(1)?,
                    file_path: row.get(2)?,
                    language: row.get(3)?,
                    kind: row.get(4)?,
                    access_count: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0),
                    last_accessed_at: row.get(6)?,
                })
            })?;

            for row in rows {
                let record = row?;
                records.insert(record.symbol_id.clone(), record);
            }
        }

        Ok(records)
    }
    pub fn get_symbol_record(&self, symbol_id: &str) -> Result<Option<SymbolRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE id = ?1
            "#,
        )?;

        let record = stmt
            .query_row(params![symbol_id], |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    qualified_name: row.get(4)?,
                    signature_fingerprint: row.get(5)?,
                    last_seen_at: row.get(6)?,
                })
            })
            .optional()?;

        Ok(record)
    }
    pub fn get_symbol_by_qualified_name(
        &self,
        qualified_name: &str,
    ) -> Result<Option<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE qualified_name = ?1
            ORDER BY id ASC
            LIMIT 1
            "#,
        )?;

        let record = stmt
            .query_row(params![qualified_name], |row| {
                Ok(SymbolRecord {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    qualified_name: row.get(4)?,
                    signature_fingerprint: row.get(5)?,
                    last_seen_at: row.get(6)?,
                })
            })
            .optional()?;

        Ok(record)
    }
    pub fn find_symbol_search_results_by_qualified_name(
        &self,
        qualified_name: &str,
    ) -> Result<Vec<SymbolSearchResult>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, qualified_name, file_path, language, kind, access_count, last_accessed_at
            FROM symbols
            WHERE qualified_name = ?1
            ORDER BY file_path ASC, kind ASC, id ASC
            "#,
        )?;

        let rows = stmt.query_map(params![qualified_name], |row| {
            Ok(SymbolSearchResult {
                symbol_id: row.get(0)?,
                qualified_name: row.get(1)?,
                file_path: row.get(2)?,
                language: row.get(3)?,
                kind: row.get(4)?,
                access_count: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0),
                last_accessed_at: row.get(6)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn list_all_symbol_ids(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id
            FROM symbols
            ORDER BY id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
    pub fn get_symbol_metadata(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SymbolMetadata>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, kind, file_path, qualified_name
            FROM symbols
            WHERE id = ?1
            LIMIT 1
            "#,
        )?;
        let row = stmt
            .query_row(params![symbol_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .optional()?;
        drop(stmt);
        drop(conn);

        let Some((symbol_id, kind, file_path, qualified_name)) = row else {
            return Ok(None);
        };
        let full_path = self
            .workspace_root()
            .map(|workspace| workspace.join(&file_path));
        let line_count = full_path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|source| source.lines().count())
            .unwrap_or(0);
        let is_public = full_path
            .as_ref()
            .map(|path| infer_symbol_is_public(path, &qualified_name))
            .unwrap_or(false);

        Ok(Some(SymbolMetadata {
            symbol_id,
            kind,
            file_path,
            is_public,
            line_count,
        }))
    }
    pub fn list_module_file_paths(
        &self,
        module_path: &str,
        language: &str,
    ) -> Result<Vec<String>, StoreError> {
        let module_path = normalize_path(module_path.trim().trim_end_matches('/'));
        let language = language.trim();
        if module_path.is_empty() || language.is_empty() {
            return Ok(Vec::new());
        }

        let like_pattern = format!("{module_path}/%");
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT DISTINCT file_path
            FROM symbols
            WHERE language = ?1
              AND (file_path = ?2 OR file_path LIKE ?3)
            ORDER BY file_path ASC
            "#,
        )?;

        let rows = stmt.query_map(params![language, module_path, like_pattern], |row| {
            row.get::<_, String>(0)
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_upsert_symbol(&self, record: SymbolRecord) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            r#"
            INSERT INTO symbols (
                id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                file_path = excluded.file_path,
                language = excluded.language,
                kind = excluded.kind,
                qualified_name = excluded.qualified_name,
                signature_fingerprint = excluded.signature_fingerprint,
                last_seen_at = excluded.last_seen_at
            "#,
            params![
                record.id,
                record.file_path,
                record.language,
                record.kind,
                record.qualified_name,
                record.signature_fingerprint,
                record.last_seen_at,
            ],
        )?;

        Ok(())
    }
    pub(crate) fn store_list_symbols_for_file(
        &self,
        file_path: &str,
    ) -> Result<Vec<SymbolRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            WHERE file_path = ?1
            ORDER BY id
            "#,
        )?;

        let rows = stmt.query_map(params![file_path], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                qualified_name: row.get(4)?,
                signature_fingerprint: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_search_symbols(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SymbolSearchResult>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let capped_limit = limit.clamp(1, 100) as i64;
        let pattern = format!("%{query}%");

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, qualified_name, file_path, language, kind, access_count, last_accessed_at
            FROM symbols
            WHERE LOWER(id) LIKE LOWER(?1)
               OR LOWER(qualified_name) LIKE LOWER(?1)
               OR LOWER(file_path) LIKE LOWER(?1)
               OR LOWER(language) LIKE LOWER(?1)
               OR LOWER(kind) LIKE LOWER(?1)
            ORDER BY qualified_name ASC, id ASC
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![pattern, capped_limit], |row| {
            Ok(SymbolSearchResult {
                symbol_id: row.get(0)?,
                qualified_name: row.get(1)?,
                file_path: row.get(2)?,
                language: row.get(3)?,
                kind: row.get(4)?,
                access_count: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0),
                last_accessed_at: row.get(6)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_increment_symbol_access(
        &self,
        symbol_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        if symbol_ids.is_empty() {
            return Ok(());
        }

        let accessed_at = accessed_at.max(0);
        let unique_ids = symbol_ids
            .iter()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .collect::<HashSet<_>>();
        if unique_ids.is_empty() {
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
                UPDATE symbols
                SET access_count = CASE
                        WHEN access_count < ?2 THEN access_count + 1
                        ELSE ?2
                    END,
                    last_accessed_at = ?3
                WHERE id = ?1
                "#,
            )?;

            for symbol_id in unique_ids {
                stmt.execute(params![symbol_id, SYMBOL_ACCESS_COUNTER_MAX, accessed_at])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn store_increment_symbol_access_debounced(
        &self,
        symbol_ids: &[String],
        accessed_at: i64,
    ) -> Result<(), StoreError> {
        if symbol_ids.is_empty() {
            return Ok(());
        }

        let now = Instant::now();
        let debounce_window = Duration::from_secs(SYMBOL_ACCESS_DEBOUNCE_SECONDS);
        let mut tracker = self.symbol_access_debounce.lock().map_err(|err| {
            StoreError::Io(std::io::Error::other(format!(
                "symbol access debounce lock poisoned: {err}"
            )))
        })?;
        tracker.retain(|_, last_accessed| {
            now.saturating_duration_since(*last_accessed) < debounce_window
        });

        let mut symbol_ids_to_increment = Vec::new();
        for symbol_id in symbol_ids {
            let trimmed = symbol_id.trim();
            if trimmed.is_empty() {
                continue;
            }

            let should_increment = tracker
                .get(trimmed)
                .map(|last_accessed| {
                    now.saturating_duration_since(*last_accessed) >= debounce_window
                })
                .unwrap_or(true);

            if !should_increment {
                continue;
            }

            tracker.insert(trimmed.to_owned(), now);
            symbol_ids_to_increment.push(trimmed.to_owned());
        }
        drop(tracker);

        self.increment_symbol_access(symbol_ids_to_increment.as_slice(), accessed_at)
    }
}
