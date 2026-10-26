use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, params};

use super::{
    GraphStore, ResolvedEdge, StoreError, SymbolRecord, run_migrations, same_language_family_sql,
};

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
    fn call_chain_sync(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() || depth == 0 {
            return Ok(Vec::new());
        }

        // Traverse unique frontiers instead of enumerating paths in a recursive
        // CTE. Shared callees and cycles otherwise multiply intermediate rows even
        // though callers only need each symbol's shortest distance from the root.
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        let mut visited = HashSet::from([symbol_id.to_owned()]);
        let mut frontier = vec![symbol_id.to_owned()];
        let mut levels = Vec::new();
        for _ in 0..depth {
            let mut level = Vec::new();
            // Keep below SQLite's parameter limit, including older builds whose
            // default is 999. A wide call graph must not fail at one frontier.
            for sources in frontier.chunks(500) {
                let placeholders = vec!["?"; sources.len()].join(", ");
                let mut stmt = tx.prepare(&format!(
                    r#"
                    SELECT DISTINCT
                        target.id,
                        target.file_path,
                        target.language,
                        target.kind,
                        target.qualified_name,
                        target.signature_fingerprint,
                        target.last_seen_at
                    FROM symbol_edges e
                    JOIN symbols source ON source.id = e.source_id
                    JOIN symbols target
                      ON target.qualified_name = e.target_qualified_name
                     AND {family}
                    WHERE e.edge_kind = 'calls'
                      AND e.source_id IN ({placeholders})
                    "#,
                    family = same_language_family_sql("source.language", "target.language"),
                ))?;
                let rows = stmt.query_map(rusqlite::params_from_iter(sources), |row| {
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
                for row in rows {
                    let record = row?;
                    if visited.insert(record.id.clone()) {
                        level.push(record);
                    }
                }
            }
            if level.is_empty() {
                break;
            }
            level.sort_by(|left, right| {
                left.qualified_name
                    .cmp(&right.qualified_name)
                    .then_with(|| left.id.cmp(&right.id))
            });
            frontier = level.iter().map(|record| record.id.clone()).collect();
            levels.push(level);
        }
        // All frontiers observe one database snapshot even if the indexer writes
        // between queries. A read-only graph connection supports this transaction.
        tx.commit()?;

        Ok(levels)
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

        // A call edge to the name counts unless every symbol with that name is outside
        // the caller's language family. The languages of those symbols are read once.
        let conn = self.connection()?;
        let mut stmt = conn.prepare(&format!(
            r#"
            WITH target_languages(language) AS MATERIALIZED (
                SELECT DISTINCT language FROM symbols WHERE qualified_name = ?1
            )
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
                JOIN symbols caller ON caller.id = e.source_id
                WHERE e.edge_kind = 'calls'
                  AND e.target_qualified_name = ?1
                  AND (
                      NOT EXISTS (SELECT 1 FROM target_languages)
                      OR EXISTS (
                          SELECT 1 FROM target_languages target
                          WHERE {family}
                      )
                  )
            ) callers
            JOIN symbols s ON s.id = callers.caller_id
            ORDER BY s.qualified_name ASC, s.id ASC
            "#,
            family = same_language_family_sql("caller.language", "target.language"),
        ))?;
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
        let reader = Self {
            sqlite_path: self.sqlite_path.clone(),
            read_only: self.read_only,
        };
        let symbol_id = symbol_id.to_owned();
        let read = move || reader.call_chain_sync(&symbol_id, depth);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(read).await.map_err(|err| {
                StoreError::Graph(format!("SQLite call-chain read worker failed: {err}"))
            })?
        } else {
            // Keep GraphStore usable from other executors without doing SQLite
            // I/O on their executor thread or requiring a new Tokio runtime.
            let (sender, receiver) = futures::channel::oneshot::channel();
            std::thread::Builder::new()
                .name("aether-graph-read".to_owned())
                .spawn(move || {
                    let _ = sender.send(read());
                })
                .map_err(|err| {
                    StoreError::Graph(format!("failed to start SQLite graph read worker: {err}"))
                })?;
            receiver.await.map_err(|err| {
                StoreError::Graph(format!("SQLite call-chain read worker stopped: {err}"))
            })?
        }
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
    async fn call_chain_waiting_for_sqlite_does_not_block_the_async_executor()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = crate::SqliteStore::open(temp.path())?;
        store.upsert_symbol(symbol("root", "root"))?;
        drop(store);
        let graph = SqliteGraphStore::open_readonly(temp.path())?;
        let blocker = Connection::open(temp.path().join(".aether/meta.sqlite"))?;
        blocker.pragma_update(None, "journal_mode", "DELETE")?;
        blocker.execute_batch("BEGIN EXCLUSIVE;")?;

        let (started, ready) = tokio::sync::oneshot::channel();
        let reader = tokio::spawn(async move {
            let _ = started.send(());
            graph.get_call_chain("root", 1).await
        });
        // With blocking SQLite I/O on this current-thread executor, the reader
        // would time out on the lock before this task could release it.
        ready.await?;
        blocker.execute_batch("COMMIT;")?;
        assert!(reader.await??.is_empty());
        Ok(())
    }

    #[test]
    fn call_chain_supports_executors_without_a_tokio_runtime()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let store = crate::SqliteStore::open(temp.path())?;
        store.upsert_symbol(symbol("root", "root"))?;
        store.upsert_symbol(symbol("callee", "callee"))?;
        store.upsert_edges(&[SymbolEdge {
            source_id: "root".to_owned(),
            target_qualified_name: "callee".to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_owned(),
        }])?;
        let graph = SqliteGraphStore::open_readonly(temp.path())?;
        assert_eq!(
            futures::executor::block_on(graph.get_call_chain("root", 1))?,
            vec![vec![symbol("callee", "callee")]]
        );
        Ok(())
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

    #[tokio::test]
    async fn sqlite_call_chain_uses_shortest_hops_and_excludes_recursive_root() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = SqliteGraphStore::open_readonly(temp.path()).expect("open readonly graph");
        for name in ["root", "alpha", "beta", "shared", "direct", "leaf"] {
            store
                .upsert_symbol(symbol(name, name))
                .expect("upsert symbol");
        }
        let edges = [
            ("root", "root"),
            ("root", "beta"),
            ("root", "alpha"),
            ("root", "direct"),
            ("alpha", "shared"),
            ("beta", "shared"),
            ("shared", "alpha"),
            ("shared", "root"),
            ("shared", "direct"),
            ("shared", "leaf"),
        ]
        .map(|(source, target)| SymbolEdge {
            source_id: source.to_owned(),
            target_qualified_name: target.to_owned(),
            edge_kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_owned(),
        });
        store.upsert_edges(&edges).expect("upsert cyclic diamond");

        for (depth, expected) in [
            (0, vec![]),
            (1, vec![vec!["alpha", "beta", "direct"]]),
            (2, vec![vec!["alpha", "beta", "direct"], vec!["shared"]]),
            (
                12,
                vec![
                    vec!["alpha", "beta", "direct"],
                    vec!["shared"],
                    vec!["leaf"],
                ],
            ),
        ] {
            let chain = graph
                .get_call_chain("root", depth)
                .await
                .expect("traverse cyclic diamond");
            let ids = chain
                .iter()
                .map(|level| level.iter().map(|row| row.id.as_str()).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            assert_eq!(ids, expected, "depth {depth}");
        }
    }

    #[tokio::test]
    async fn sqlite_call_chain_batches_wide_frontiers_without_duplicate_callees() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = SqliteGraphStore::open(temp.path()).expect("open sqlite graph");
        store
            .upsert_symbol(symbol("root", "root"))
            .expect("upsert root");
        store
            .upsert_symbol(symbol("leaf", "leaf"))
            .expect("upsert leaf");
        let mut edges = Vec::new();
        for index in 0..510 {
            let name = format!("branch-{index:03}");
            store
                .upsert_symbol(symbol(&name, &name))
                .expect("upsert branch");
            edges.push(SymbolEdge {
                source_id: "root".to_owned(),
                target_qualified_name: name.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            });
            edges.push(SymbolEdge {
                source_id: name,
                target_qualified_name: "leaf".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            });
        }
        store.upsert_edges(&edges).expect("upsert wide diamond");

        let chain = graph
            .get_call_chain("root", 3)
            .await
            .expect("traverse wide graph");
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].len(), 510);
        assert_eq!(chain[0][0].id, "branch-000");
        assert_eq!(chain[0][509].id, "branch-509");
        assert_eq!(chain[1].len(), 1);
        assert_eq!(chain[1][0].id, "leaf");
    }
}
