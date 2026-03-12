use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirHistoryRecord {
    pub symbol_id: String,
    pub version: i64,
    pub sir_hash: String,
    pub provider: String,
    pub model: String,
    pub created_at: i64,
    pub sir_json: String,
    pub commit_hash: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SirHistorySelector {
    Version(i64),
    CreatedAt(i64),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirHistoryResolvedPair {
    pub from: SirHistoryRecord,
    pub to: SirHistoryRecord,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SirVersionWriteResult {
    pub version: i64,
    pub updated_at: i64,
    pub changed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SirHistoryBaselineSelector {
    Version(i64),
    CreatedAt(i64),
    CommitHash(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SirHistoryTransferRecord {
    pub(crate) sir_hash: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) created_at: i64,
    pub(crate) sir_json: String,
    pub(crate) commit_hash: Option<String>,
}
pub(crate) fn normalize_commit_hash(commit_hash: Option<&str>) -> Option<String> {
    let value = commit_hash?.trim();
    if value.len() != 40 {
        return None;
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }

    Some(value.to_owned())
}
pub(crate) fn normalize_commit_hash_for_like(commit_hash: &str) -> String {
    commit_hash
        .trim()
        .to_ascii_lowercase()
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .collect()
}
pub(crate) fn resolve_history_selector_index(
    history: &[SirHistoryRecord],
    selector: &SirHistorySelector,
) -> Option<usize> {
    match selector {
        SirHistorySelector::Version(version) => {
            history.iter().position(|record| record.version == *version)
        }
        SirHistorySelector::CreatedAt(created_at) => history
            .iter()
            .enumerate()
            .filter(|(_, record)| record.created_at <= *created_at)
            .map(|(idx, _)| idx)
            .next_back(),
    }
}
pub(crate) fn load_sir_history_transfer_records(
    tx: &Transaction<'_>,
    symbol_id: &str,
) -> Result<Vec<SirHistoryTransferRecord>, StoreError> {
    let mut stmt = tx.prepare(
        r#"
        SELECT sir_hash, provider, model, created_at, sir_json, commit_hash
        FROM sir_history
        WHERE symbol_id = ?1
        ORDER BY version ASC, created_at ASC
        "#,
    )?;
    let rows = stmt.query_map(params![symbol_id], |row| {
        Ok(SirHistoryTransferRecord {
            sir_hash: row.get(0)?,
            provider: row.get(1)?,
            model: row.get(2)?,
            created_at: row.get::<_, i64>(3)?.max(0),
            sir_json: row.get(4)?,
            commit_hash: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}
pub(crate) fn load_max_sir_history_version(
    tx: &Transaction<'_>,
    symbol_id: &str,
) -> Result<i64, StoreError> {
    tx.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM sir_history WHERE symbol_id = ?1",
        params![symbol_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value.max(0))
    .map_err(Into::into)
}
pub(crate) fn append_sir_history_records(
    tx: &Transaction<'_>,
    symbol_id: &str,
    base_version: i64,
    records: &[SirHistoryTransferRecord],
) -> Result<Option<i64>, StoreError> {
    if records.is_empty() {
        return Ok(None);
    }

    let mut stmt = tx.prepare(
        r#"
        INSERT INTO sir_history (
            symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
    )?;

    let mut latest_version = None;
    for (offset, record) in records.iter().enumerate() {
        let version = base_version + offset as i64 + 1;
        stmt.execute(params![
            symbol_id,
            version,
            &record.sir_hash,
            &record.provider,
            &record.model,
            record.created_at,
            &record.sir_json,
            &record.commit_hash,
        ])?;
        latest_version = Some(version);
    }

    Ok(latest_version)
}

impl SqliteStore {
    pub(crate) fn store_list_sir_history(
        &self,
        symbol_id: &str,
    ) -> Result<Vec<SirHistoryRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
            FROM sir_history
            WHERE symbol_id = ?1
            ORDER BY version ASC
            "#,
        )?;

        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok(SirHistoryRecord {
                symbol_id: row.get(0)?,
                version: row.get(1)?,
                sir_hash: row.get(2)?,
                provider: row.get(3)?,
                model: row.get(4)?,
                created_at: row.get(5)?,
                sir_json: row.get(6)?,
                commit_hash: row.get(7)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_latest_sir_history_pair(
        &self,
        symbol_id: &str,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError> {
        let history = self.list_sir_history(symbol_id)?;
        let Some(latest) = history.last().cloned() else {
            return Ok(None);
        };
        let from = history
            .get(history.len().saturating_sub(2))
            .cloned()
            .unwrap_or_else(|| latest.clone());

        Ok(Some(SirHistoryResolvedPair { from, to: latest }))
    }
    pub(crate) fn store_resolve_sir_history_pair(
        &self,
        symbol_id: &str,
        from: SirHistorySelector,
        to: SirHistorySelector,
    ) -> Result<Option<SirHistoryResolvedPair>, StoreError> {
        let history = self.list_sir_history(symbol_id)?;
        let from_idx = resolve_history_selector_index(&history, &from);
        let to_idx = resolve_history_selector_index(&history, &to);

        let (Some(from_idx), Some(to_idx)) = (from_idx, to_idx) else {
            return Ok(None);
        };
        if from_idx > to_idx {
            return Ok(None);
        }

        Ok(Some(SirHistoryResolvedPair {
            from: history[from_idx].clone(),
            to: history[to_idx].clone(),
        }))
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn store_record_sir_version_if_changed(
        &self,
        symbol_id: &str,
        sir_hash: &str,
        provider: &str,
        model: &str,
        sir_json: &str,
        created_at: i64,
        commit_hash: Option<&str>,
    ) -> Result<SirVersionWriteResult, StoreError> {
        let created_at = created_at.max(0);
        let commit_hash = normalize_commit_hash(commit_hash);
        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;

        let write_result = {
            let mut latest_stmt = tx.prepare(
                r#"
                SELECT version, sir_hash, created_at
                FROM sir_history
                WHERE symbol_id = ?1
                ORDER BY version DESC
                LIMIT 1
                "#,
            )?;

            let latest = latest_stmt
                .query_row(params![symbol_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .optional()?;

            if let Some((latest_version, latest_hash, _latest_created_at)) = latest {
                if latest_hash == sir_hash {
                    SirVersionWriteResult {
                        version: latest_version,
                        updated_at: created_at,
                        changed: false,
                    }
                } else {
                    let next_version = latest_version + 1;
                    tx.execute(
                        r#"
                        INSERT INTO sir_history (
                            symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                        )
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                        "#,
                        params![
                            symbol_id,
                            next_version,
                            sir_hash,
                            provider,
                            model,
                            created_at,
                            sir_json,
                            commit_hash.as_deref(),
                        ],
                    )?;

                    SirVersionWriteResult {
                        version: next_version,
                        updated_at: created_at,
                        changed: true,
                    }
                }
            } else {
                tx.execute(
                    r#"
                    INSERT INTO sir_history (
                        symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                    )
                    VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7)
                    "#,
                    params![
                        symbol_id,
                        sir_hash,
                        provider,
                        model,
                        created_at,
                        sir_json,
                        commit_hash.as_deref(),
                    ],
                )?;

                SirVersionWriteResult {
                    version: 1,
                    updated_at: created_at,
                    changed: true,
                }
            }
        };

        tx.commit()?;
        Ok(write_result)
    }
    pub(crate) fn store_resolve_sir_baseline_by_selector(
        &self,
        symbol_id: &str,
        selector: SirHistoryBaselineSelector,
    ) -> Result<Option<SirHistoryRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(None);
        }

        let sql = match selector {
            SirHistoryBaselineSelector::Version(_) => {
                r#"
                SELECT symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                FROM sir_history
                WHERE symbol_id = ?1
                  AND version = ?2
                LIMIT 1
                "#
            }
            SirHistoryBaselineSelector::CreatedAt(_) => {
                r#"
                SELECT symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                FROM sir_history
                WHERE symbol_id = ?1
                  AND created_at <= ?2
                ORDER BY created_at DESC, version DESC
                LIMIT 1
                "#
            }
            SirHistoryBaselineSelector::CommitHash(_) => {
                r#"
                SELECT symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
                FROM sir_history
                WHERE symbol_id = ?1
                  AND commit_hash IS NOT NULL
                  AND LOWER(commit_hash) LIKE ?2
                ORDER BY created_at DESC, version DESC
                LIMIT 1
                "#
            }
        };

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let record = match selector {
            SirHistoryBaselineSelector::Version(version) => stmt
                .query_row(params![symbol_id, version], |row| {
                    Ok(SirHistoryRecord {
                        symbol_id: row.get(0)?,
                        version: row.get(1)?,
                        sir_hash: row.get(2)?,
                        provider: row.get(3)?,
                        model: row.get(4)?,
                        created_at: row.get(5)?,
                        sir_json: row.get(6)?,
                        commit_hash: row.get(7)?,
                    })
                })
                .optional()?,
            SirHistoryBaselineSelector::CreatedAt(created_at) => stmt
                .query_row(params![symbol_id, created_at.max(0)], |row| {
                    Ok(SirHistoryRecord {
                        symbol_id: row.get(0)?,
                        version: row.get(1)?,
                        sir_hash: row.get(2)?,
                        provider: row.get(3)?,
                        model: row.get(4)?,
                        created_at: row.get(5)?,
                        sir_json: row.get(6)?,
                        commit_hash: row.get(7)?,
                    })
                })
                .optional()?,
            SirHistoryBaselineSelector::CommitHash(commit_hash) => {
                let prefix = normalize_commit_hash_for_like(commit_hash.as_str());
                if prefix.is_empty() {
                    return Ok(None);
                }
                stmt.query_row(params![symbol_id, format!("{prefix}%")], |row| {
                    Ok(SirHistoryRecord {
                        symbol_id: row.get(0)?,
                        version: row.get(1)?,
                        sir_hash: row.get(2)?,
                        provider: row.get(3)?,
                        model: row.get(4)?,
                        created_at: row.get(5)?,
                        sir_json: row.get(6)?,
                        commit_hash: row.get(7)?,
                    })
                })
                .optional()?
            }
        };

        Ok(record)
    }
}
