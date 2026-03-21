use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaVersion {
    pub component: String,
    pub version: u32,
    pub migrated_at: i64,
}
pub(crate) fn run_migrations(conn: &Connection) -> Result<(), StoreError> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if version < 1 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS symbols (
            id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            language TEXT NOT NULL,
            kind TEXT NOT NULL,
            qualified_name TEXT NOT NULL,
            signature_fingerprint TEXT NOT NULL,
            last_seen_at INTEGER NOT NULL,
            access_count INTEGER NOT NULL DEFAULT 0,
            last_accessed_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS sir (
            id TEXT PRIMARY KEY,
            sir_hash TEXT NOT NULL,
            sir_version INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            sir_json TEXT
        );

        CREATE TABLE IF NOT EXISTS sir_history (
            symbol_id TEXT NOT NULL,
            version INTEGER NOT NULL CHECK (version >= 1),
            sir_hash TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            sir_json TEXT NOT NULL,
            commit_hash TEXT CHECK (
                commit_hash IS NULL
                OR (
                    LENGTH(commit_hash) = 40
                    AND commit_hash NOT GLOB '*[^0-9a-f]*'
                )
            ),
            PRIMARY KEY (symbol_id, version)
        );

        CREATE INDEX IF NOT EXISTS idx_sir_history_symbol_created_version
            ON sir_history(symbol_id, created_at ASC, version ASC);

        CREATE INDEX IF NOT EXISTS idx_sir_history_symbol_latest
            ON sir_history(symbol_id, version DESC);

        CREATE TABLE IF NOT EXISTS sir_embeddings (
            symbol_id TEXT PRIMARY KEY,
            sir_hash TEXT NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            embedding_dim INTEGER NOT NULL,
            embedding_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_sir_embeddings_provider_model_dim
            ON sir_embeddings(provider, model, embedding_dim);

        CREATE TABLE IF NOT EXISTS threshold_calibration (
            language TEXT PRIMARY KEY,
            threshold REAL NOT NULL,
            sample_size INTEGER NOT NULL,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            calibrated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_threshold_calibration_provider_model
            ON threshold_calibration(provider, model);

        CREATE TABLE IF NOT EXISTS schema_version (
            component TEXT PRIMARY KEY,
            version INTEGER NOT NULL,
            migrated_at INTEGER NOT NULL
        );

        INSERT OR IGNORE INTO schema_version (component, version, migrated_at)
        VALUES ('core', 1, unixepoch());

        CREATE TABLE IF NOT EXISTS project_notes (
            note_id TEXT PRIMARY KEY,
            content TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            source_type TEXT NOT NULL,
            source_agent TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            entity_refs TEXT NOT NULL DEFAULT '[]',
            file_refs TEXT NOT NULL DEFAULT '[]',
            symbol_refs TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            access_count INTEGER NOT NULL DEFAULT 0,
            last_accessed_at INTEGER,
            is_archived INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_project_notes_content_hash
            ON project_notes(content_hash);
        CREATE INDEX IF NOT EXISTS idx_project_notes_source_type
            ON project_notes(source_type);
        CREATE INDEX IF NOT EXISTS idx_project_notes_created_at
            ON project_notes(created_at);
        CREATE INDEX IF NOT EXISTS idx_project_notes_archived
            ON project_notes(is_archived);

        CREATE TABLE IF NOT EXISTS project_notes_embeddings (
            note_id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            embedding_dim INTEGER NOT NULL,
            embedding_json TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_project_notes_embeddings_provider_model_dim
            ON project_notes_embeddings(provider, model, embedding_dim);

        CREATE TABLE IF NOT EXISTS symbol_edges (
            source_id TEXT NOT NULL,
            target_qualified_name TEXT NOT NULL,
            edge_kind TEXT NOT NULL CHECK (edge_kind IN ('calls', 'depends_on', 'type_ref', 'implements')),
            file_path TEXT NOT NULL,
            PRIMARY KEY (source_id, target_qualified_name, edge_kind)
        );

        CREATE INDEX IF NOT EXISTS idx_edges_target
            ON symbol_edges(target_qualified_name);

        CREATE INDEX IF NOT EXISTS idx_edges_file
            ON symbol_edges(file_path);

        CREATE TABLE IF NOT EXISTS coupling_mining_state (
            id INTEGER PRIMARY KEY DEFAULT 1,
            last_commit_hash TEXT,
            last_mined_at INTEGER,
            commits_scanned INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS drift_analysis_state (
            id INTEGER PRIMARY KEY DEFAULT 1,
            last_analysis_commit TEXT,
            last_analysis_at INTEGER,
            symbols_analyzed INTEGER NOT NULL DEFAULT 0,
            drift_detected INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS drift_results (
            result_id TEXT PRIMARY KEY,
            symbol_id TEXT NOT NULL,
            file_path TEXT NOT NULL,
            symbol_name TEXT NOT NULL,
            drift_type TEXT NOT NULL,
            drift_magnitude REAL,
            current_sir_hash TEXT,
            baseline_sir_hash TEXT,
            commit_range_start TEXT,
            commit_range_end TEXT,
            drift_summary TEXT,
            detail_json TEXT NOT NULL,
            detected_at INTEGER NOT NULL,
            is_acknowledged INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_drift_results_type
            ON drift_results(drift_type);
        CREATE INDEX IF NOT EXISTS idx_drift_results_file
            ON drift_results(file_path);
        CREATE INDEX IF NOT EXISTS idx_drift_results_ack
            ON drift_results(is_acknowledged);

        CREATE TABLE IF NOT EXISTS community_snapshot (
            snapshot_id TEXT NOT NULL,
            symbol_id TEXT NOT NULL,
            community_id INTEGER NOT NULL,
            captured_at INTEGER NOT NULL,
            PRIMARY KEY (snapshot_id, symbol_id)
        );

        CREATE INDEX IF NOT EXISTS idx_community_snapshot_symbol
            ON community_snapshot(symbol_id);
        CREATE INDEX IF NOT EXISTS idx_community_snapshot_captured
            ON community_snapshot(captured_at);

        CREATE TABLE IF NOT EXISTS test_intents (
            intent_id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            test_name TEXT NOT NULL,
            intent_text TEXT NOT NULL,
            group_label TEXT,
            language TEXT NOT NULL,
            symbol_id TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_test_intents_file
            ON test_intents(file_path);
        "#,
        )?;

        if !table_has_column(conn, "sir", "sir_json")? {
            conn.execute("ALTER TABLE sir ADD COLUMN sir_json TEXT", [])?;
        }

        ensure_sir_column(conn, "sir_status", "TEXT NOT NULL DEFAULT 'fresh'")?;
        ensure_sir_column(conn, "last_error", "TEXT")?;
        ensure_sir_column(conn, "last_attempt_at", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_sir_column(conn, "generation_pass", "TEXT DEFAULT 'scan'")?;
        ensure_sir_history_column(conn, "commit_hash", "TEXT")?;
        ensure_symbols_column(conn, "access_count", "INTEGER NOT NULL DEFAULT 0")?;
        ensure_symbols_column(conn, "last_accessed_at", "INTEGER")?;

        conn.execute(
            "UPDATE sir SET sir_status = 'fresh' WHERE COALESCE(TRIM(sir_status), '') = ''",
            [],
        )?;
        conn.execute(
            "UPDATE sir SET last_attempt_at = updated_at WHERE last_attempt_at = 0",
            [],
        )?;
        conn.execute(
            r#"
        INSERT INTO sir_history (
            symbol_id, version, sir_hash, provider, model, created_at, sir_json, commit_hash
        )
        SELECT
            s.id,
            CASE WHEN s.sir_version > 0 THEN s.sir_version ELSE 1 END,
            s.sir_hash,
            s.provider,
            s.model,
            CASE WHEN s.updated_at > 0 THEN s.updated_at ELSE unixepoch() END,
            s.sir_json,
            NULL
        FROM sir s
        WHERE COALESCE(TRIM(s.sir_hash), '') <> ''
          AND COALESCE(TRIM(s.sir_json), '') <> ''
          AND NOT EXISTS (
              SELECT 1 FROM sir_history h WHERE h.symbol_id = s.id
          )
        "#,
            [],
        )?;

        conn.execute("PRAGMA user_version = 1", [])?;
    }

    if version < 2 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS document_units (
            unit_id          TEXT PRIMARY KEY,
            domain           TEXT NOT NULL,
            unit_kind        TEXT NOT NULL,
            display_name     TEXT NOT NULL,
            content          TEXT NOT NULL,
            source_path      TEXT NOT NULL,
            byte_range_start INTEGER NOT NULL,
            byte_range_end   INTEGER NOT NULL,
            parent_id        TEXT,
            metadata_json    TEXT NOT NULL DEFAULT '{}',
            created_at       INTEGER NOT NULL,
            updated_at       INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_document_units_domain ON document_units(domain);
        CREATE INDEX IF NOT EXISTS idx_document_units_source ON document_units(source_path);
        CREATE INDEX IF NOT EXISTS idx_document_units_kind ON document_units(domain, unit_kind);

        CREATE TABLE IF NOT EXISTS semantic_records (
            record_id       TEXT PRIMARY KEY,
            unit_id         TEXT NOT NULL,
            domain          TEXT NOT NULL,
            schema_name     TEXT NOT NULL,
            schema_version  TEXT NOT NULL,
            content_hash    TEXT NOT NULL,
            record_json     TEXT NOT NULL,
            embedding_text  TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            FOREIGN KEY (unit_id) REFERENCES document_units(unit_id)
        );
        CREATE INDEX IF NOT EXISTS idx_semantic_records_unit ON semantic_records(unit_id);
        CREATE INDEX IF NOT EXISTS idx_semantic_records_domain ON semantic_records(domain);
        CREATE INDEX IF NOT EXISTS idx_semantic_records_schema ON semantic_records(domain, schema_name);
        "#,
        )?;
        conn.execute("PRAGMA user_version = 2", [])?;
    }

    if version < 3 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS write_intents (
            intent_id TEXT PRIMARY KEY,
            symbol_id TEXT NOT NULL,
            file_path TEXT NOT NULL,
            operation TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            payload_json TEXT,
            created_at INTEGER NOT NULL,
            completed_at INTEGER,
            error_message TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_write_intents_status
            ON write_intents(status);
        CREATE INDEX IF NOT EXISTS idx_write_intents_created
            ON write_intents(created_at);
        "#,
        )?;
        conn.execute("PRAGMA user_version = 3", [])?;
    }

    if version < 4 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS sir_requests (
            symbol_id TEXT PRIMARY KEY,
            requested_at INTEGER NOT NULL,
            request_count INTEGER NOT NULL DEFAULT 1
        );

        CREATE INDEX IF NOT EXISTS idx_sir_requests_requested_at
            ON sir_requests(requested_at);
        "#,
        )?;
        conn.execute("PRAGMA user_version = 4", [])?;
    }

    if version < 5 {
        if table_exists(conn, "sir")? {
            ensure_sir_column(conn, "generation_pass", "TEXT DEFAULT 'scan'")?;
            conn.execute(
                "UPDATE sir SET generation_pass = 'scan' WHERE COALESCE(TRIM(generation_pass), '') = ''",
                [],
            )?;
        }
        conn.execute("PRAGMA user_version = 5", [])?;
    }

    if version < 6 {
        if table_exists(conn, "sir")? {
            ensure_sir_column(conn, "generation_pass", "TEXT DEFAULT 'scan'")?;
            conn.execute(
                "UPDATE sir SET generation_pass = 'scan' WHERE generation_pass IN ('triage', 'single')",
                [],
            )?;
            conn.execute(
                "UPDATE sir SET generation_pass = 'scan' WHERE COALESCE(TRIM(generation_pass), '') = ''",
                [],
            )?;
        }
        conn.execute("PRAGMA user_version = 6", [])?;
    }

    if version < 7 {
        upgrade_symbol_edges_table(conn)?;
        conn.execute("PRAGMA user_version = 7", [])?;
    }

    if version < 8 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS intent_snapshots (
            snapshot_id   TEXT PRIMARY KEY,
            git_commit    TEXT NOT NULL,
            created_at    INTEGER NOT NULL,
            scope         TEXT NOT NULL,
            symbol_count  INTEGER NOT NULL,
            deep_count    INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_intent_snapshots_created
            ON intent_snapshots(created_at DESC, snapshot_id DESC);

        CREATE TABLE IF NOT EXISTS intent_snapshot_entries (
            snapshot_id           TEXT NOT NULL,
            symbol_id             TEXT NOT NULL,
            qualified_name        TEXT NOT NULL,
            file_path             TEXT NOT NULL,
            signature_fingerprint TEXT NOT NULL,
            sir_json              TEXT NOT NULL,
            generation_pass       TEXT NOT NULL,
            was_deep_scanned      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (snapshot_id, symbol_id),
            FOREIGN KEY (snapshot_id) REFERENCES intent_snapshots(snapshot_id)
        );

        CREATE INDEX IF NOT EXISTS idx_intent_snapshot_entries_snapshot
            ON intent_snapshot_entries(snapshot_id, file_path, qualified_name, symbol_id);
        "#,
        )?;
        conn.execute("PRAGMA user_version = 8", [])?;
    }

    if version < 9 {
        ensure_sir_column(conn, "prompt_hash", "TEXT")?;

        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS sir_fingerprint_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            symbol_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            prompt_hash TEXT NOT NULL,
            prompt_hash_previous TEXT,
            trigger TEXT NOT NULL,
            source_changed INTEGER NOT NULL DEFAULT 0,
            neighbor_changed INTEGER NOT NULL DEFAULT 0,
            config_changed INTEGER NOT NULL DEFAULT 0,
            generation_model TEXT,
            generation_pass TEXT,
            delta_sem REAL
        );

        CREATE INDEX IF NOT EXISTS idx_fingerprint_symbol_time
            ON sir_fingerprint_history(symbol_id, timestamp DESC);

        CREATE INDEX IF NOT EXISTS idx_fingerprint_delta
            ON sir_fingerprint_history(delta_sem DESC)
            WHERE delta_sem IS NOT NULL;
        "#,
        )?;
        conn.execute("PRAGMA user_version = 9", [])?;
    }

    if version < 10 {
        ensure_sir_column(conn, "staleness_score", "REAL")?;
        conn.execute("PRAGMA user_version = 10", [])?;
    }

    if version < 11 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS task_context_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_description TEXT NOT NULL,
            branch_name TEXT,
            resolved_symbol_ids TEXT NOT NULL,
            resolved_file_paths TEXT NOT NULL,
            total_symbols INTEGER NOT NULL,
            budget_used INTEGER NOT NULL,
            budget_max INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_task_context_history_created
            ON task_context_history(created_at DESC);
        "#,
        )?;
        conn.execute("PRAGMA user_version = 11", [])?;
    }

    if version < 12 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS metrics_seismograph (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            batch_timestamp INTEGER NOT NULL,
            codebase_shift REAL NOT NULL,
            semantic_velocity REAL NOT NULL,
            symbols_regenerated INTEGER NOT NULL,
            symbols_above_noise INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_metrics_seismograph_time
            ON metrics_seismograph(batch_timestamp DESC);

        CREATE TABLE IF NOT EXISTS metrics_community_stability (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            community_id TEXT NOT NULL,
            computed_at INTEGER NOT NULL,
            stability REAL NOT NULL,
            symbol_count INTEGER NOT NULL,
            breach_count INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_metrics_community_stability_time
            ON metrics_community_stability(computed_at DESC);

        CREATE TABLE IF NOT EXISTS metrics_cascade (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            epicenter_symbol_id TEXT NOT NULL,
            chain_json TEXT NOT NULL,
            total_hops INTEGER NOT NULL,
            max_delta_sem REAL NOT NULL,
            detected_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_metrics_cascade_time
            ON metrics_cascade(detected_at DESC);

        CREATE TABLE IF NOT EXISTS metrics_aftershock_model (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            trained_at INTEGER NOT NULL,
            training_samples INTEGER NOT NULL,
            weights_json TEXT NOT NULL,
            auc_roc REAL
        );
        "#,
        )?;
        conn.execute("PRAGMA user_version = 12", [])?;
    }

    if version < 13 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS intent_contracts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            symbol_id TEXT NOT NULL,
            clause_type TEXT NOT NULL,
            clause_text TEXT NOT NULL,
            clause_embedding_json TEXT,
            created_at INTEGER NOT NULL,
            created_by TEXT NOT NULL,
            active INTEGER NOT NULL DEFAULT 1,
            violation_streak INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_contracts_symbol
            ON intent_contracts(symbol_id) WHERE active = 1;

        CREATE TABLE IF NOT EXISTS intent_violations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            contract_id INTEGER NOT NULL,
            symbol_id TEXT NOT NULL,
            sir_version INTEGER NOT NULL,
            violation_type TEXT NOT NULL,
            confidence REAL,
            reason TEXT,
            detected_at INTEGER NOT NULL,
            dismissed INTEGER NOT NULL DEFAULT 0,
            dismissed_at INTEGER,
            dismissed_reason TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_violations_contract
            ON intent_violations(contract_id, detected_at DESC);

        CREATE INDEX IF NOT EXISTS idx_violations_symbol
            ON intent_violations(symbol_id, detected_at DESC);
        "#,
        )?;
        conn.execute("PRAGMA user_version = 13", [])?;
    }

    if version < 14 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS sir_quality (
            sir_id TEXT PRIMARY KEY REFERENCES sir(id),
            specificity REAL NOT NULL,
            behavioral_depth REAL NOT NULL,
            error_coverage REAL NOT NULL,
            length_score REAL NOT NULL,
            composite_quality REAL NOT NULL,
            confidence_percentile REAL NOT NULL,
            normalized_quality REAL NOT NULL,
            computed_at INTEGER NOT NULL
        );
        "#,
        )?;
        conn.execute("PRAGMA user_version = 14", [])?;
    }

    if version < 15 {
        conn.execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS symbol_neighbors (
            symbol_id     TEXT NOT NULL,
            neighbor_id   TEXT NOT NULL,
            edge_type     TEXT NOT NULL,
            neighbor_name TEXT NOT NULL,
            neighbor_file TEXT NOT NULL,
            PRIMARY KEY (symbol_id, neighbor_id, edge_type)
        );
        CREATE INDEX IF NOT EXISTS idx_neighbors_symbol ON symbol_neighbors(symbol_id);
        CREATE INDEX IF NOT EXISTS idx_neighbors_neighbor ON symbol_neighbors(neighbor_id);
        CREATE INDEX IF NOT EXISTS idx_neighbors_file ON symbol_neighbors(neighbor_file);
        "#,
        )?;
        rebuild_symbol_neighbors(conn)?;
        conn.execute("PRAGMA user_version = 15", [])?;
    }

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_version (
            component TEXT PRIMARY KEY,
            version INTEGER NOT NULL,
            migrated_at INTEGER NOT NULL
        );
        "#,
    )?;
    let current_user_version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    conn.execute(
        r#"
        INSERT INTO schema_version (component, version, migrated_at)
        VALUES ('core', ?1, unixepoch())
        ON CONFLICT(component) DO UPDATE SET
            version = excluded.version,
            migrated_at = CASE
                WHEN schema_version.version <> excluded.version THEN excluded.migrated_at
                ELSE schema_version.migrated_at
            END
        "#,
        params![current_user_version],
    )?;

    Ok(())
}
fn rebuild_symbol_neighbors(conn: &Connection) -> Result<(), StoreError> {
    if !table_exists(conn, "symbol_neighbors")?
        || !table_exists(conn, "symbol_edges")?
        || !table_exists(conn, "symbols")?
    {
        return Ok(());
    }

    conn.execute("DELETE FROM symbol_neighbors", [])?;
    conn.execute_batch(
        r#"
        INSERT OR REPLACE INTO symbol_neighbors (
            symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file
        )
        SELECT
            e.source_id,
            s_target.id,
            e.edge_kind,
            s_target.qualified_name,
            s_target.file_path
        FROM symbol_edges e
        JOIN symbols s_source ON s_source.id = e.source_id
        JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name;

        INSERT OR REPLACE INTO symbol_neighbors (
            symbol_id, neighbor_id, edge_type, neighbor_name, neighbor_file
        )
        SELECT
            s_target.id,
            e.source_id,
            CASE e.edge_kind
                WHEN 'calls' THEN 'called_by'
                WHEN 'depends_on' THEN 'depended_on_by'
                WHEN 'implements' THEN 'implemented_by'
                WHEN 'type_ref' THEN 'type_ref_by'
                ELSE e.edge_kind || '_reverse'
            END,
            s_source.qualified_name,
            s_source.file_path
        FROM symbol_edges e
        JOIN symbols s_source ON s_source.id = e.source_id
        JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name;
        "#,
    )?;

    Ok(())
}
fn ensure_sir_column(
    conn: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), StoreError> {
    if !table_exists(conn, "sir")? {
        return Ok(());
    }

    if table_has_column(conn, "sir", column_name)? {
        return Ok(());
    }

    let sql = format!("ALTER TABLE sir ADD COLUMN {column_name} {column_definition}");
    conn.execute(&sql, [])?;
    Ok(())
}
fn ensure_sir_history_column(
    conn: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), StoreError> {
    if table_has_column(conn, "sir_history", column_name)? {
        return Ok(());
    }

    let sql = format!("ALTER TABLE sir_history ADD COLUMN {column_name} {column_definition}");
    conn.execute(&sql, [])?;
    Ok(())
}
fn ensure_symbols_column(
    conn: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), StoreError> {
    if table_has_column(conn, "symbols", column_name)? {
        return Ok(());
    }

    let sql = format!("ALTER TABLE symbols ADD COLUMN {column_name} {column_definition}");
    conn.execute(&sql, [])?;
    Ok(())
}
fn upgrade_symbol_edges_table(conn: &Connection) -> Result<(), StoreError> {
    if !table_exists(conn, "symbol_edges")? {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        ALTER TABLE symbol_edges RENAME TO symbol_edges_old;

        CREATE TABLE symbol_edges (
            source_id TEXT NOT NULL,
            target_qualified_name TEXT NOT NULL,
            edge_kind TEXT NOT NULL CHECK (edge_kind IN ('calls', 'depends_on', 'type_ref', 'implements')),
            file_path TEXT NOT NULL,
            PRIMARY KEY (source_id, target_qualified_name, edge_kind)
        );

        INSERT INTO symbol_edges (source_id, target_qualified_name, edge_kind, file_path)
        SELECT source_id, target_qualified_name, edge_kind, file_path
        FROM symbol_edges_old;

        DROP TABLE symbol_edges_old;

        CREATE INDEX IF NOT EXISTS idx_edges_target
            ON symbol_edges(target_qualified_name);

        CREATE INDEX IF NOT EXISTS idx_edges_file
            ON symbol_edges(file_path);
        "#,
    )?;

    Ok(())
}
fn table_has_column(
    conn: &Connection,
    table_name: &str,
    column_name: &str,
) -> Result<bool, StoreError> {
    let sql = format!("PRAGMA table_info({table_name})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;

    for row in rows {
        if row?.eq_ignore_ascii_case(column_name) {
            return Ok(true);
        }
    }

    Ok(false)
}
fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, StoreError> {
    let mut stmt = conn.prepare(
        r#"
        SELECT 1
        FROM sqlite_master
        WHERE type = 'table' AND name = ?1
        LIMIT 1
        "#,
    )?;
    let row = stmt
        .query_row(params![table_name], |row| row.get::<_, i64>(0))
        .optional()?;
    Ok(row.is_some())
}

impl SqliteStore {
    pub fn get_schema_version(&self) -> Result<SchemaVersion, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT component, version, migrated_at
            FROM schema_version
            WHERE component = 'core'
            "#,
            [],
            |row| {
                Ok(SchemaVersion {
                    component: row.get(0)?,
                    version: row.get::<_, i64>(1)? as u32,
                    migrated_at: row.get(2)?,
                })
            },
        )
        .map_err(Into::into)
    }
    pub fn check_compatibility(
        &self,
        component: &str,
        max_supported: u32,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let found = conn
            .query_row(
                r#"
                SELECT version
                FROM schema_version
                WHERE component = ?1
                "#,
                params![component],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        match found {
            Some(version) if version >= 0 && (version as u32) <= max_supported => Ok(()),
            Some(version) => Err(StoreError::Compatibility(format!(
                "component '{component}' schema version {version} exceeds max supported {max_supported}"
            ))),
            None => Err(StoreError::Compatibility(format!(
                "missing schema_version row for component '{component}'"
            ))),
        }
    }
}
