//! Shared SQL target proof used by every persisted graph projection and reader.
//!
//! Rust and ECMAScript names omit their defining file. A matching spelling alone
//! is therefore not evidence of a relationship, even when only one match exists.

use super::{StoreError, same_language_family_sql};
use aether_core::ECMASCRIPT_LANGUAGES;
use rusqlite::Connection;

const GRAPH_PROVENANCE_SCHEMA_VERSION: u32 = 24;

fn supported_graph_schema_version(conn: &Connection) -> Result<u32, StoreError> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    super::schema::ensure_supported_core_version(version)?;
    Ok(version as u32)
}

impl super::SqliteStore {
    /// Whether the actual catalog schema can express graph target provenance.
    /// This read-only capability check does not consult migration receipt metadata
    /// or turn unsupported future schemas and database errors into missing features.
    pub fn has_graph_provenance_schema(&self) -> Result<bool, StoreError> {
        let conn = self.lock_connection()?;
        Ok(supported_graph_schema_version(&conn)? >= GRAPH_PROVENANCE_SCHEMA_VERSION)
    }
}

/// Older read-only catalogs remain usable for search and SIRs, but their graph
/// shape cannot express proof and their cached neighbors may contain false links.
pub(crate) fn require_graph_provenance_schema(conn: &Connection) -> Result<(), StoreError> {
    let version = supported_graph_schema_version(conn)?;
    if version < GRAPH_PROVENANCE_SCHEMA_VERSION {
        return Err(StoreError::Compatibility(format!(
            "graph target provenance requires core schema {GRAPH_PROVENANCE_SCHEMA_VERSION} (found {version}); open the workspace writable and reindex before reading graph relationships"
        )));
    }
    Ok(())
}

/// Whether a writable opening has purged old persistent Surreal edges and
/// replayed the SQL evidence. A pending/absent receipt must not expose old links.
pub(crate) fn graph_projection_provenance_ready(
    root: &std::path::Path,
) -> Result<bool, StoreError> {
    let path = root.join(".aether/meta.sqlite");
    if !path.exists() {
        return Ok(false);
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < 24 {
        return Ok(false);
    }
    use rusqlite::OptionalExtension;
    let receipt = conn
        .query_row(
            "SELECT version FROM schema_version WHERE component = 'graph_target_provenance'",
            [],
            |row| row.get::<_, u32>(0),
        )
        .optional()?;
    Ok(receipt == Some(1))
}

pub(crate) fn require_graph_projection_provenance(
    root: &std::path::Path,
) -> Result<(), StoreError> {
    // Readonly graph opening historically initializes an absent graph. There is
    // no old projection to distrust in that case; writable initialization still
    // replays the catalog and completes its receipt before returning the handle.
    if !root.join(".aether/graph").try_exists()? {
        let catalog = root.join(".aether/meta.sqlite");
        if catalog.try_exists()? {
            let conn =
                Connection::open_with_flags(catalog, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            // Initializing a graph may bootstrap a current or entirely absent
            // catalog, but a readonly caller must never migrate an older one.
            require_graph_provenance_schema(&conn)?;
        }
        return Ok(());
    }
    if !graph_projection_provenance_ready(root)? {
        return Err(StoreError::Compatibility(
            "persistent graph target provenance needs rebuilding; run writable indexing/startup before reading graph relationships".to_owned(),
        ));
    }
    Ok(())
}

struct GraphProjectionRebuildPlan {
    symbols: Vec<super::SymbolRecord>,
    edges: Vec<super::ResolvedEdge>,
}

/// Read one SQL snapshot off the async executor. Migration replay must not hold
/// the catalog connection lock while awaiting the separate graph backend.
fn graph_projection_rebuild_plan(
    root: &std::path::Path,
    force_rebuild: bool,
) -> Result<Option<GraphProjectionRebuildPlan>, StoreError> {
    if !force_rebuild && graph_projection_provenance_ready(root)? {
        return Ok(None);
    }
    let catalog = super::SqliteStore::open(root)?;
    let conn = catalog.lock_connection()?;
    if force_rebuild {
        // A SQL receipt describes a previous projection. A newly initialized
        // graph must replay even when a catalog was restored with receipt 1.
        conn.execute(
            "UPDATE schema_version SET version = 0, migrated_at = unixepoch() WHERE component = 'graph_target_provenance'",
            [],
        )?;
    }
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Deferred)?;
    let mut stmt = tx.prepare(
        "SELECT id, file_path, language, kind, qualified_name, signature_fingerprint, last_seen_at FROM symbols ORDER BY id",
    )?;
    let symbols = stmt
        .query_map([], |row| {
            Ok(super::SymbolRecord {
                id: row.get(0)?,
                file_path: row.get(1)?,
                language: row.get(2)?,
                kind: row.get(3)?,
                qualified_name: row.get(4)?,
                signature_fingerprint: row.get(5)?,
                last_seen_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let resolution = edge_target_matches_sql("e", "source.language", "target");
    let mut stmt = tx.prepare(&format!(
        "SELECT DISTINCT e.source_id, target.id, e.edge_kind, e.file_path FROM symbol_edges e \
         JOIN symbols source ON source.id = e.source_id JOIN symbols target ON {resolution} \
         WHERE e.edge_kind IN ('calls','depends_on','type_ref','implements') \
         ORDER BY e.source_id, target.id, e.edge_kind"
    ))?;
    let mut edges = Vec::new();
    for row in stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })? {
        let (source_id, target_id, kind, file_path) = row?;
        let edge_kind = super::graph::edge_kind_from_str(&kind).ok_or_else(|| {
            StoreError::Compatibility(format!(
                "unsupported edge kind during provenance rebuild: {kind}"
            ))
        })?;
        edges.push(super::ResolvedEdge {
            source_id,
            target_id,
            edge_kind,
            file_path,
        });
    }
    drop(stmt);
    tx.commit()?;
    Ok(Some(GraphProjectionRebuildPlan { symbols, edges }))
}

/// Called under Surreal's per-store setup lock. The pending receipt survives a
/// failure, so a later writable opening retries and readonly consumers fail closed.
pub(crate) async fn rebuild_graph_projection_provenance(
    root: &std::path::Path,
    graph: &super::SurrealGraphStore,
    force_rebuild: bool,
) -> Result<(), StoreError> {
    let task_root = root.to_path_buf();
    let plan = tokio::task::spawn_blocking(move || {
        graph_projection_rebuild_plan(&task_root, force_rebuild)
    })
    .await
    .map_err(|err| {
        StoreError::Graph(format!("graph provenance snapshot worker failed: {err}"))
    })??;
    let Some(GraphProjectionRebuildPlan { symbols, edges }) = plan else {
        return Ok(());
    };
    // Remove every old structural edge, including edges of files no longer in
    // SQLite. Other graph relations (coupling, test evidence) are unaffected.
    graph
        .db()
        .query("DELETE depends_on;")
        .await
        .and_then(|response| response.check())
        .map_err(|err| {
            StoreError::Graph(format!(
                "failed to clear graph edges for provenance rebuild: {err}"
            ))
        })?;
    use super::GraphStore;
    // Publish every target first so file order cannot lose a legitimate edge.
    for symbol in &symbols {
        graph.upsert_symbol_node(symbol).await?;
    }
    for edge in &edges {
        graph.upsert_edge(edge).await?;
    }
    let task_root = root.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
        let catalog = super::SqliteStore::open(task_root)?;
        catalog.lock_connection()?.execute(
            "UPDATE schema_version SET version = 1, migrated_at = unixepoch() WHERE component = 'graph_target_provenance'",
            [],
        )?;
        Ok(())
    }).await.map_err(|err| StoreError::Graph(format!("graph provenance receipt worker failed: {err}")))??;
    Ok(())
}

/// Candidate matching before rejecting ambiguous file-scoped targets. Coverage
/// uses this to disclose ambiguity while graph readers fail closed on it.
pub(crate) fn edge_target_candidate_matches_sql(
    edge: &str,
    source_language: &str,
    target: &str,
) -> String {
    let protected = std::iter::once("rust")
        .chain(ECMASCRIPT_LANGUAGES.iter().copied())
        .map(|language| format!("'{language}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let family = same_language_family_sql(source_language, &format!("{target}.language"));
    format!(
        "({target}.qualified_name = {edge}.target_qualified_name AND {family} AND (\
         ({edge}.target_symbol_id IS NOT NULL AND {target}.id = {edge}.target_symbol_id \
          AND ({edge}.target_file_path IS NULL OR {target}.file_path = {edge}.target_file_path)) OR \
         ({edge}.target_symbol_id IS NULL AND {edge}.target_file_path IS NOT NULL \
          AND {target}.file_path = {edge}.target_file_path) OR \
         ({edge}.target_symbol_id IS NULL AND {edge}.target_file_path IS NULL \
          AND {source_language} NOT IN ({protected}))))"
    )
}

/// A proven ID, a unique target in a proven file, or the legacy name semantics
/// for languages whose provenance extraction has not been changed yet.
pub(crate) fn edge_target_matches_sql(edge: &str, source_language: &str, target: &str) -> String {
    let candidates = edge_target_candidate_matches_sql(edge, source_language, target);
    let other_family = same_language_family_sql(source_language, "other_target.language");
    format!(
        "({candidates} AND ({edge}.target_symbol_id IS NOT NULL OR \
         {edge}.target_file_path IS NULL OR NOT EXISTS (\
         SELECT 1 FROM symbols other_target WHERE \
         other_target.file_path = {edge}.target_file_path AND \
         other_target.qualified_name = {edge}.target_qualified_name AND \
         other_target.id <> {target}.id AND {other_family})))"
    )
}

#[cfg(test)]
mod tests {
    use crate::{
        EdgeKind, GraphStore, SqliteGraphStore, SqliteStore, SymbolCatalogStore, SymbolEdge,
        SymbolRecord, SymbolRelationStore,
    };
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn symbol(id: &str, file: &str, language: &str, name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: file.to_owned(),
            language: language.to_owned(),
            kind: "function".to_owned(),
            qualified_name: name.to_owned(),
            signature_fingerprint: id.to_owned(),
            last_seen_at: 1,
        }
    }

    fn edge(source: &str, name: &str, id: Option<&str>, target_file: Option<&str>) -> SymbolEdge {
        SymbolEdge {
            source_id: source.to_owned(),
            target_qualified_name: name.to_owned(),
            target_symbol_id: id.map(str::to_owned),
            target_file_path: target_file.map(str::to_owned),
            edge_kind: EdgeKind::Calls,
            file_path: "project/main.ts".to_owned(),
        }
    }

    #[tokio::test]
    async fn proof_identity_preserves_distinct_targets_and_rejects_stale_or_ambiguous_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = SqliteStore::open(temp.path())?;
        for row in [
            symbol("caller", "project/main.ts", "typescript", "caller"),
            symbol("one", "project/one.js", "javascript", "helper"),
            symbol("two", "project/two.ts", "typescript", "helper"),
            symbol(
                "sole-unrelated",
                "elsewhere/helper.ts",
                "typescript",
                "unrelated",
            ),
            symbol("wrong-family", "project/one.js", "python", "foreign"),
            symbol(
                "overload-a",
                "project/ambiguous.ts",
                "typescript",
                "ambiguous",
            ),
            symbol(
                "overload-b",
                "project/ambiguous.ts",
                "typescript",
                "ambiguous",
            ),
        ] {
            store.upsert_symbol(row)?;
        }
        let edges = vec![
            edge("caller", "helper", None, Some("project/one.js")),
            edge("caller", "helper", None, Some("project/two.ts")),
            // Neither a sole global match nor a stale ID plus a valid file proves a target.
            edge("caller", "unrelated", None, None),
            edge("caller", "helper", Some("removed"), Some("project/one.js")),
            edge("caller", "foreign", Some("wrong-family"), None),
            edge("caller", "ambiguous", None, Some("project/ambiguous.ts")),
        ];
        store.upsert_edges(&edges)?;
        assert_eq!(
            store
                .list_symbol_edges_for_source_and_kinds("caller", &[EdgeKind::Calls])?
                .len(),
            6
        );
        let mut resolved = store
            .list_graph_dependency_edges()?
            .into_iter()
            .map(|edge| edge.target_symbol_id)
            .collect::<Vec<_>>();
        resolved.sort();
        assert_eq!(resolved, ["one", "two"]);
        store.populate_symbol_neighbors("project/main.ts")?;
        assert_eq!(store.get_symbol_neighbors("caller")?.len(), 2);
        let graph = SqliteGraphStore::open_readonly(temp.path())?;
        assert_eq!(graph.get_call_chain("caller", 1).await?[0].len(), 2);
        assert!(graph.get_callers("unrelated").await?.is_empty());
        assert!(graph.get_callers("foreign").await?.is_empty());
        assert!(graph.get_callers("ambiguous").await?.is_empty());
        let coverage = store.call_edge_coverage_for_sources(&["caller".to_owned()])?;
        assert_eq!(
            (
                coverage.call_edges,
                coverage.resolved,
                coverage.ambiguous,
                coverage.unresolved
            ),
            (6, 2, 1, 3)
        );
        Ok(())
    }

    #[test]
    fn migration_preserves_raw_references_and_removes_unproved_cached_links()
    -> Result<(), Box<dyn std::error::Error>> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            r#"
            CREATE TABLE symbols (
                id TEXT PRIMARY KEY, file_path TEXT NOT NULL, language TEXT NOT NULL,
                kind TEXT NOT NULL, qualified_name TEXT NOT NULL,
                signature_fingerprint TEXT NOT NULL, last_seen_at INTEGER NOT NULL
            );
            CREATE TABLE symbol_edges (
                source_id TEXT NOT NULL, target_qualified_name TEXT NOT NULL,
                edge_kind TEXT NOT NULL, file_path TEXT NOT NULL,
                PRIMARY KEY(source_id, target_qualified_name, edge_kind)
            );
            CREATE TABLE symbol_neighbors (
                symbol_id TEXT NOT NULL, neighbor_id TEXT NOT NULL, edge_type TEXT NOT NULL,
                neighbor_name TEXT NOT NULL, neighbor_file TEXT NOT NULL,
                PRIMARY KEY(symbol_id, neighbor_id, edge_type)
            );
            INSERT INTO symbols VALUES
                ('caller','a.ts','typescript','function','caller','a',1),
                ('target','unrelated.ts','typescript','function','helper','b',1),
                ('py-caller','a.py','python','function','py_caller','c',1),
                ('py-target','b.py','python','function','module::helper','d',1);
            INSERT INTO symbol_edges VALUES
                ('caller','helper','calls','a.ts'),
                ('py-caller','module::helper','calls','a.py');
            INSERT INTO symbol_neighbors VALUES ('caller','target','calls','helper','unrelated.ts');
            PRAGMA user_version = 23;
        "#,
        )?;
        crate::run_migrations(&conn)?;
        let raw: i64 = conn.query_row("SELECT COUNT(*) FROM symbol_edges", [], |row| row.get(0))?;
        let unproved: i64 = conn.query_row("SELECT COUNT(*) FROM symbol_edges WHERE target_symbol_id IS NULL AND target_file_path IS NULL", [], |row| row.get(0))?;
        let protected: i64 = conn.query_row("SELECT COUNT(*) FROM symbol_neighbors WHERE symbol_id = 'caller' OR neighbor_id = 'caller'", [], |row| row.get(0))?;
        let legacy: i64 = conn.query_row("SELECT COUNT(*) FROM symbol_neighbors WHERE symbol_id = 'py-caller' AND neighbor_id = 'py-target'", [], |row| row.get(0))?;
        assert_eq!((raw, unproved, protected, legacy), (2, 2, 0, 1));
        // Opening the already-migrated store does not reinterpret legacy evidence.
        crate::run_migrations(&conn)?;
        Ok(())
    }

    #[tokio::test]
    async fn persistent_legacy_edges_are_rebuilt_before_readonly_graph_access()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = SqliteStore::open(temp.path())?;
        let graph = crate::SurrealGraphStore::open(temp.path()).await?;
        for row in [
            symbol("caller", "project/main.ts", "typescript", "caller"),
            symbol("unrelated", "elsewhere/helper.ts", "typescript", "helper"),
            symbol("py-caller", "a.py", "python", "py_caller"),
            symbol("py-target", "z.py", "python", "module::helper"),
        ] {
            graph.upsert_symbol_node(&row).await?;
            store.upsert_symbol(row)?;
        }
        store.upsert_edges(&[
            edge("caller", "helper", None, None),
            SymbolEdge {
                source_id: "py-caller".to_owned(),
                target_qualified_name: "module::helper".to_owned(),
                target_symbol_id: None,
                target_file_path: None,
                edge_kind: EdgeKind::Calls,
                file_path: "a.py".to_owned(),
            },
        ])?;
        graph
            .upsert_edge(&crate::ResolvedEdge {
                source_id: "caller".to_owned(),
                target_id: "unrelated".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "project/main.ts".to_owned(),
            })
            .await?;
        store.lock_connection()?.execute(
            "UPDATE schema_version SET version = 0 WHERE component = 'graph_target_provenance'",
            [],
        )?;
        assert!(matches!(
            crate::SurrealGraphStore::open_readonly(temp.path()).await,
            Err(crate::StoreError::Compatibility(_))
        ));
        let rebuilt = crate::SurrealGraphStore::open(temp.path()).await?;
        assert!(rebuilt.get_dependencies("caller").await?.is_empty());
        assert_eq!(
            rebuilt.get_dependencies("py-caller").await?[0].id,
            "py-target"
        );
        assert!(super::graph_projection_provenance_ready(temp.path())?);
        assert!(
            crate::SurrealGraphStore::open_readonly(temp.path())
                .await
                .is_ok()
        );
        Ok(())
    }

    #[tokio::test]
    async fn readonly_older_catalog_allows_search_and_refuses_unproved_graphs()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = SqliteStore::open(temp.path())?;
        store.upsert_symbol(symbol("caller", "project/main.ts", "typescript", "caller"))?;
        drop(store);
        let conn = Connection::open(temp.path().join(".aether/meta.sqlite"))?;
        conn.pragma_update(None, "user_version", 23)?;
        drop(conn);
        let store = SqliteStore::open_readonly(temp.path())?;
        assert_eq!(store.search_symbols("caller", 10)?.len(), 1);
        assert!(matches!(
            store.get_symbol_neighbors("caller"),
            Err(crate::StoreError::Compatibility(_))
        ));
        assert!(matches!(
            store.list_callers_for_symbol("caller"),
            Err(crate::StoreError::Compatibility(_))
        ));
        let graph = SqliteGraphStore::open_readonly(temp.path())?;
        assert!(matches!(
            graph.get_call_chain("caller", 1).await,
            Err(crate::StoreError::Compatibility(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn readonly_initializes_absent_graph_without_accepting_unproved_catalog_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = SqliteStore::open(temp.path())?;
        store.upsert_symbol(symbol("caller", "project/main.ts", "typescript", "caller"))?;
        store.upsert_symbol(symbol(
            "unrelated",
            "other/helper.ts",
            "typescript",
            "helper",
        ))?;
        store.upsert_edges(&[edge("caller", "helper", None, None)])?;
        assert!(!temp.path().join(".aether/graph").exists());
        assert!(!super::graph_projection_provenance_ready(temp.path())?);
        let graph = crate::SurrealGraphStore::open_readonly(temp.path()).await?;
        assert!(graph.get_dependencies("caller").await?.is_empty());
        assert!(super::graph_projection_provenance_ready(temp.path())?);
        Ok(())
    }

    #[tokio::test]
    async fn readonly_absent_graph_does_not_migrate_an_existing_legacy_catalog()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        std::fs::create_dir_all(temp.path().join(".aether"))?;
        let path = temp.path().join(".aether/meta.sqlite");
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            r#"
            CREATE TABLE symbol_edges (
                source_id TEXT NOT NULL, target_qualified_name TEXT NOT NULL,
                edge_kind TEXT NOT NULL, file_path TEXT NOT NULL,
                PRIMARY KEY (source_id, target_qualified_name, edge_kind)
            );
            INSERT INTO symbol_edges VALUES ('caller','helper','calls','main.ts');
            PRAGMA user_version = 23;
        "#,
        )?;
        drop(conn);
        let result = crate::SurrealGraphStore::open_readonly(temp.path()).await;
        assert!(
            matches!(result, Err(crate::StoreError::Compatibility(message)) if message.contains("core schema 24"))
        );
        assert!(!temp.path().join(".aether/graph").exists());
        let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let columns: u32 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('symbol_edges')",
            [],
            |row| row.get(0),
        )?;
        let references: u32 =
            conn.query_row("SELECT COUNT(*) FROM symbol_edges", [], |row| row.get(0))?;
        assert_eq!((version, columns, references), (23, 4, 1));
        Ok(())
    }

    #[tokio::test]
    async fn fresh_graph_replays_catalog_even_when_its_prior_projection_receipt_is_complete()
    -> Result<(), Box<dyn std::error::Error>> {
        for empty_directory in [false, true] {
            let temp = tempdir()?;
            let store = SqliteStore::open(temp.path())?;
            store.upsert_symbol(symbol("caller", "project/main.ts", "typescript", "caller"))?;
            store.upsert_symbol(symbol(
                "helper",
                "project/helper.ts",
                "typescript",
                "helper",
            ))?;
            store.upsert_edges(&[edge("caller", "helper", Some("helper"), None)])?;
            store.lock_connection()?.execute(
                "UPDATE schema_version SET version = 1 WHERE component = 'graph_target_provenance'",
                [],
            )?;
            if empty_directory {
                std::fs::create_dir_all(temp.path().join(".aether/graph"))?;
            }
            let graph = crate::SurrealGraphStore::open_readonly(temp.path()).await?;
            assert_eq!(graph.get_dependencies("caller").await?[0].id, "helper");
            assert_eq!(graph.list_all_symbol_ids().await?.len(), 2);
            assert!(super::graph_projection_provenance_ready(temp.path())?);
        }
        Ok(())
    }

    #[test]
    fn graph_schema_capability_uses_actual_supported_schema_without_migrating()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = SqliteStore::open(temp.path())?;
        let conn = Connection::open(temp.path().join(".aether/meta.sqlite"))?;
        // The metadata receipt deliberately stays current while the actual schema
        // version varies, as legacy readonly consumers must use the actual shape.
        for version in [0, 23] {
            conn.pragma_update(None, "user_version", version)?;
            assert!(!store.has_graph_provenance_schema()?);
            let actual: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            assert_eq!(actual, version);
        }
        conn.pragma_update(None, "user_version", crate::CORE_SCHEMA_VERSION)?;
        assert!(store.has_graph_provenance_schema()?);
        conn.pragma_update(None, "user_version", crate::CORE_SCHEMA_VERSION + 1)?;
        assert!(
            matches!(store.has_graph_provenance_schema(), Err(crate::StoreError::Compatibility(message)) if message.contains("unsupported core schema version"))
        );
        Ok(())
    }
}
