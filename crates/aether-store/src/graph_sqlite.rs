use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, params};

use super::{GraphStore, ResolvedEdge, StoreError, SymbolRecord, run_migrations};

pub struct SqliteGraphStore {
    sqlite_path: PathBuf,
    read_only: bool,
}

impl SqliteGraphStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_internal(workspace_root, false)
    }

    pub fn open_readonly(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_internal(workspace_root, true)
    }

    fn open_internal(
        workspace_root: impl AsRef<Path>,
        read_only: bool,
    ) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let aether_dir = workspace_root.join(".aether");
        let sqlite_path = aether_dir.join("meta.sqlite");

        if !read_only {
            fs::create_dir_all(&aether_dir)?;
        }

        let conn = if read_only {
            Connection::open_with_flags(&sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        } else {
            Connection::open(&sqlite_path)?
        };
        conn.busy_timeout(Duration::from_secs(5))?;
        if !read_only {
            run_migrations(&conn)?;
        }
        drop(conn);

        Ok(Self {
            sqlite_path,
            read_only,
        })
    }

    fn connection(&self) -> Result<Connection, StoreError> {
        let conn = if self.read_only {
            Connection::open_with_flags(&self.sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?
        } else {
            Connection::open(&self.sqlite_path)?
        };
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(conn)
    }
}

#[async_trait]
impl GraphStore for SqliteGraphStore {
    async fn upsert_symbol_node(&self, _symbol: &SymbolRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn upsert_edge(&self, _edge: &ResolvedEdge) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT DISTINCT
                s.id,
                s.file_path,
                s.language,
                s.kind,
                s.qualified_name,
                s.signature_fingerprint,
                s.last_seen_at
            FROM (
                SELECT n.neighbor_id AS caller_id
                FROM symbols target
                JOIN symbol_neighbors n ON n.symbol_id = target.id
                WHERE target.qualified_name = ?1
                  AND n.edge_type = 'called_by'
                UNION
                SELECT e.source_id AS caller_id
                FROM symbol_edges e
                WHERE e.edge_kind = 'calls'
                  AND e.target_qualified_name = ?1
            ) callers
            JOIN symbols s ON s.id = callers.caller_id
            ORDER BY s.qualified_name ASC, s.id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![qualified_name], |row| {
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

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    async fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.id,
                s.file_path,
                s.language,
                s.kind,
                s.qualified_name,
                s.signature_fingerprint,
                s.last_seen_at
            FROM symbol_neighbors n
            JOIN symbols s ON s.id = n.neighbor_id
            WHERE n.symbol_id = ?1
              AND n.edge_type = 'calls'
            ORDER BY s.qualified_name ASC, s.id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
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

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    async fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() || depth == 0 {
            return Ok(Vec::new());
        }

        let conn = self.connection()?;
        let mut stmt = conn.prepare(
            r#"
            WITH RECURSIVE reachable(symbol_id, depth) AS (
                SELECT target.id, 1
                FROM symbol_edges e
                JOIN symbols target ON target.qualified_name = e.target_qualified_name
                WHERE e.edge_kind = 'calls'
                  AND e.source_id = ?1
                UNION ALL
                SELECT target.id, reachable.depth + 1
                FROM reachable
                JOIN symbol_edges e
                  ON e.source_id = reachable.symbol_id
                 AND e.edge_kind = 'calls'
                JOIN symbols target ON target.qualified_name = e.target_qualified_name
                WHERE reachable.depth < ?2
            ),
            ranked AS (
                SELECT symbol_id, MIN(depth) AS depth
                FROM reachable
                GROUP BY symbol_id
            )
            SELECT
                s.id,
                s.file_path,
                s.language,
                s.kind,
                s.qualified_name,
                s.signature_fingerprint,
                s.last_seen_at,
                ranked.depth
            FROM ranked
            JOIN symbols s ON s.id = ranked.symbol_id
            ORDER BY ranked.depth ASC, s.qualified_name ASC, s.id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![symbol_id, depth as i64], |row| {
            Ok((
                SymbolRecord {
                    id: row.get(0)?,
                    file_path: row.get(1)?,
                    language: row.get(2)?,
                    kind: row.get(3)?,
                    qualified_name: row.get(4)?,
                    signature_fingerprint: row.get(5)?,
                    last_seen_at: row.get(6)?,
                },
                row.get::<_, i64>(7)?,
            ))
        })?;

        let mut levels = Vec::new();
        let mut current_depth = 0i64;
        for row in rows {
            let (record, depth) = row?;
            if depth != current_depth {
                current_depth = depth;
                levels.push(Vec::new());
            }
            if let Some(level) = levels.last_mut() {
                level.push(record);
            }
        }

        Ok(levels)
    }

    async fn delete_edges_for_file(&self, _file_path: &str) -> Result<(), StoreError> {
        Ok(())
    }

    async fn delete_symbols_batch(&self, _symbol_ids: &[String]) -> Result<(), StoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{SymbolCatalogStore, SymbolRecord, SymbolRelationStore};
    use aether_core::{EdgeKind, SymbolEdge};

    fn symbol(id: &str, qualified_name: &str) -> SymbolRecord {
        SymbolRecord {
            id: id.to_owned(),
            file_path: "src/lib.rs".to_owned(),
            language: "rust".to_owned(),
            kind: "function".to_owned(),
            qualified_name: qualified_name.to_owned(),
            signature_fingerprint: format!("sig-{id}"),
            last_seen_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn sqlite_graph_resolves_callers_and_dependencies() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = SqliteGraphStore::open(temp.path()).expect("open sqlite graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        store.upsert_symbol(alpha.clone()).expect("upsert alpha");
        store.upsert_symbol(beta.clone()).expect("upsert beta");
        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: alpha.id.clone(),
                    target_qualified_name: "beta".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: alpha.id.clone(),
                    target_qualified_name: "external::thing".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
            ])
            .expect("upsert edges");
        store
            .populate_symbol_neighbors("src/lib.rs")
            .expect("populate symbol neighbors");

        let callers = graph.get_callers("beta").await.expect("get callers");
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].id, alpha.id);

        let deps = graph
            .get_dependencies(&alpha.id)
            .await
            .expect("get dependencies");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].id, beta.id);
    }

    #[tokio::test]
    async fn sqlite_graph_returns_multi_hop_call_chain() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = SqliteGraphStore::open(temp.path()).expect("open sqlite graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        let delta = symbol("sym-delta", "delta");

        for row in [&alpha, &beta, &gamma, &delta] {
            store.upsert_symbol(row.clone()).expect("upsert symbol");
        }
        store
            .upsert_edges(&[
                SymbolEdge {
                    source_id: alpha.id.clone(),
                    target_qualified_name: "beta".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: beta.id.clone(),
                    target_qualified_name: "gamma".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
                SymbolEdge {
                    source_id: gamma.id.clone(),
                    target_qualified_name: "delta".to_owned(),
                    edge_kind: EdgeKind::Calls,
                    file_path: "src/lib.rs".to_owned(),
                },
            ])
            .expect("upsert edges");

        let chain = graph
            .get_call_chain(&alpha.id, 3)
            .await
            .expect("get call chain at depth 3");
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0][0].id, beta.id);
        assert_eq!(chain[1][0].id, gamma.id);
        assert_eq!(chain[2][0].id, delta.id);
    }
}
