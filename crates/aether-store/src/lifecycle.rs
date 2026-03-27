use super::*;

fn normalize_symbol_ids(symbol_ids: &[String]) -> Vec<String> {
    let mut normalized = symbol_ids
        .iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}
fn normalize_reconcile_pairs(migrations: &[(String, String)]) -> Vec<(String, String)> {
    let mut normalized = migrations
        .iter()
        .map(|(old_id, new_id)| (old_id.trim().to_owned(), new_id.trim().to_owned()))
        .filter(|(old_id, new_id)| !old_id.is_empty() && !new_id.is_empty() && old_id != new_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}
fn delete_symbol_embeddings_batch(
    tx: &Transaction<'_>,
    symbol_ids: &[String],
) -> Result<(), StoreError> {
    if symbol_ids.is_empty() {
        return Ok(());
    }

    for chunk in symbol_ids.chunks(RECONCILE_PARAM_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM sir_embeddings WHERE symbol_id IN ({placeholders})");
        let params_vec = chunk
            .iter()
            .cloned()
            .map(SqlValue::Text)
            .collect::<Vec<_>>();
        tx.execute(sql.as_str(), params_from_iter(params_vec))?;
    }
    Ok(())
}
fn delete_symbol_fully(tx: &Transaction<'_>, symbol_id: &str) -> Result<(), StoreError> {
    let symbol_id = symbol_id.trim();
    if symbol_id.is_empty() {
        return Ok(());
    }

    for (table, column) in [
        ("sir_embeddings", "symbol_id"),
        ("sir_history", "symbol_id"),
        ("write_intents", "symbol_id"),
        ("sir_requests", "symbol_id"),
        ("sir_fingerprint_history", "symbol_id"),
        ("symbol_edges", "source_id"),
        ("intent_violations", "symbol_id"),
        ("intent_contracts", "symbol_id"),
        ("test_intents", "symbol_id"),
        ("community_snapshot", "symbol_id"),
        ("drift_results", "symbol_id"),
        ("sir_quality", "sir_id"),
        ("sir", "id"),
        ("symbols", "id"),
    ] {
        let sql = format!("DELETE FROM {table} WHERE {column} = ?1");
        tx.execute(sql.as_str(), params![symbol_id])?;
    }
    tx.execute(
        "DELETE FROM symbol_neighbors WHERE symbol_id = ?1 OR neighbor_id = ?1",
        params![symbol_id],
    )?;

    Ok(())
}
fn delete_symbol_records_for_ids(
    tx: &Transaction<'_>,
    symbol_ids: &[String],
) -> Result<(), StoreError> {
    let normalized = normalize_symbol_ids(symbol_ids);
    if normalized.is_empty() {
        return Ok(());
    }

    for symbol_id in &normalized {
        delete_symbol_fully(tx, symbol_id)?;
    }

    Ok(())
}

impl SqliteStore {
    pub fn list_stale_symbols(
        &self,
        snapshot_ids: &HashSet<String>,
    ) -> Result<Vec<SymbolRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at
            FROM symbols
            ORDER BY file_path ASC, qualified_name ASC, id ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
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

        let mut stale = Vec::new();
        for row in rows {
            let record = row?;
            if !snapshot_ids.contains(record.id.as_str()) {
                stale.push(record);
            }
        }
        Ok(stale)
    }
    pub fn delete_symbol_embeddings(&self, symbol_ids: &[String]) -> Result<(), StoreError> {
        let normalized = normalize_symbol_ids(symbol_ids);
        if normalized.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        delete_symbol_embeddings_batch(&tx, &normalized)?;
        tx.commit()?;
        Ok(())
    }
    pub fn reconcile_and_prune(
        &self,
        migrations: &[(String, String)],
        prunes: &[String],
    ) -> Result<(usize, usize), StoreError> {
        let migrations = normalize_reconcile_pairs(migrations);
        let prunes = normalize_symbol_ids(prunes);
        if migrations.is_empty() && prunes.is_empty() {
            return Ok((0, 0));
        }

        let mut mirror_writes = Vec::<(String, String)>::new();
        let mut mirror_deletes = Vec::<String>::new();

        let (migrated_count, pruned_count) = {
            let conn = self.conn.lock().unwrap();
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
            let mut migrated_count = 0usize;

            for (old_id, new_id) in &migrations {
                let Some(old_access) = load_symbol_access_state(&tx, old_id)? else {
                    continue;
                };
                let Some(new_access) = load_symbol_access_state(&tx, new_id)? else {
                    return Err(StoreError::Compatibility(format!(
                        "reconciliation target symbol missing from symbols table: {new_id}"
                    )));
                };

                let old_sir = load_sir_row_state(&tx, old_id)?;
                let new_sir = load_sir_row_state(&tx, new_id)?;

                let move_current_sir = match (old_sir.as_ref(), new_sir.as_ref()) {
                    (Some(_), None) => true,
                    (Some(old_row), Some(new_row)) if old_row.updated_at > new_row.updated_at => {
                        tracing::warn!(
                            old_id = %old_id,
                            new_id = %new_id,
                            old_updated_at = old_row.updated_at,
                            new_updated_at = new_row.updated_at,
                            "reconciliation conflict: migrated older target SIR to newer symbol ID"
                        );
                        true
                    }
                    (Some(old_row), Some(new_row)) => {
                        tracing::warn!(
                            old_id = %old_id,
                            new_id = %new_id,
                            old_updated_at = old_row.updated_at,
                            new_updated_at = new_row.updated_at,
                            "reconciliation conflict: keeping target SIR and discarding stale branch history"
                        );
                        false
                    }
                    (None, _) => false,
                };

                let mut old_history = load_sir_history_transfer_records(&tx, old_id)?;
                if old_history.is_empty()
                    && let Some(row) = old_sir.as_ref()
                {
                    old_history.push(SirHistoryTransferRecord {
                        sir_hash: row.sir_hash.clone(),
                        provider: row.provider.clone(),
                        model: row.model.clone(),
                        created_at: row.updated_at.max(0),
                        sir_json: row.sir_json.clone().unwrap_or_default(),
                        commit_hash: None,
                    });
                }

                let append_old_history = new_sir.is_none() || move_current_sir;
                let mut latest_migrated_version = None::<i64>;
                if append_old_history && !old_history.is_empty() {
                    let base_version = load_max_sir_history_version(&tx, new_id)?;
                    latest_migrated_version =
                        append_sir_history_records(&tx, new_id, base_version, &old_history)?;
                }

                if move_current_sir && let Some(row) = old_sir.as_ref() {
                    let sir_version =
                        latest_migrated_version.unwrap_or_else(|| row.sir_version.max(1));
                    upsert_sir_row_state(&tx, new_id, row, sir_version)?;
                    if let Some(sir_json) = row.sir_json.as_ref() {
                        mirror_writes.push((new_id.clone(), sir_json.clone()));
                    }
                }

                let merged_access = merge_symbol_access_state(new_access, old_access);
                update_symbol_access_state(&tx, new_id, &merged_access)?;
                delete_symbol_records_for_ids(&tx, std::slice::from_ref(old_id))?;
                mirror_deletes.push(old_id.clone());
                migrated_count += 1;
            }

            for chunk in prunes.chunks(RECONCILE_PARAM_CHUNK) {
                delete_symbol_records_for_ids(&tx, chunk)?;
                mirror_deletes.extend(chunk.iter().cloned());
            }

            tx.commit()?;
            (migrated_count, prunes.len())
        };

        let mut deleted = HashSet::new();
        for symbol_id in mirror_deletes {
            if !deleted.insert(symbol_id.clone()) {
                continue;
            }
            match fs::remove_file(self.sir_blob_path(symbol_id.as_str())) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => {
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        error = %err,
                        "failed to remove stale mirrored SIR file during reconciliation"
                    );
                }
            }
        }

        if self.mirror_sir_files {
            let mut written = HashSet::new();
            for (symbol_id, sir_json) in mirror_writes {
                if !written.insert(symbol_id.clone()) {
                    continue;
                }
                if let Err(err) = fs::write(self.sir_blob_path(symbol_id.as_str()), sir_json) {
                    tracing::warn!(
                        symbol_id = %symbol_id,
                        error = %err,
                        "failed to write mirrored SIR file during reconciliation"
                    );
                }
            }
        }

        Ok((migrated_count, pruned_count))
    }
    pub(crate) fn store_mark_removed(&self, symbol_id: &str) -> Result<(), StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(());
        }
        {
            let conn = self.conn.lock().unwrap();
            let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
            delete_symbol_fully(&tx, symbol_id)?;
            tx.commit()?;
        }

        let path = self.sir_blob_path(symbol_id);
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }

        Ok(())
    }
}
