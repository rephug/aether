use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CouplingEdgeRecord {
    pub file_a: String,
    pub file_b: String,
    pub co_change_count: i64,
    pub total_commits_a: i64,
    pub total_commits_b: i64,
    pub git_coupling: f32,
    pub static_signal: f32,
    pub semantic_signal: f32,
    pub fused_score: f32,
    pub coupling_type: String,
    pub last_co_change_commit: String,
    pub last_co_change_at: i64,
    pub mined_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestedByRecord {
    pub target_file: String,
    pub test_file: String,
    pub intent_count: i64,
    pub confidence: f32,
    pub inference_method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEdge {
    pub source_id: String,
    pub target_id: String,
    pub edge_kind: EdgeKind,
    pub file_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphDependencyEdgeRecord {
    pub source_symbol_id: String,
    pub target_symbol_id: String,
    pub edge_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSyncStats {
    pub resolved_edges: usize,
    pub unresolved_edges: usize,
}

pub(crate) const STRUCTURAL_EDGE_KINDS: &[&str] =
    &["calls", "depends_on", "type_ref", "implements"];

pub(crate) fn edge_kind_from_str(value: &str) -> Option<EdgeKind> {
    match value {
        "calls" => Some(EdgeKind::Calls),
        "depends_on" => Some(EdgeKind::DependsOn),
        "type_ref" => Some(EdgeKind::TypeRef),
        "implements" => Some(EdgeKind::Implements),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamDependencyNodeRecord {
    pub symbol_id: String,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamDependencyEdgeRecord {
    pub source_id: String,
    pub target_id: String,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpstreamDependencyTraversal {
    pub nodes: Vec<UpstreamDependencyNodeRecord>,
    pub edges: Vec<UpstreamDependencyEdgeRecord>,
}

impl SqliteStore {
    pub fn list_graph_dependency_edges(
        &self,
    ) -> Result<Vec<GraphDependencyEdgeRecord>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT e.source_id, target.id, e.edge_kind
            FROM symbol_edges e
            JOIN symbols target
              ON target.qualified_name = e.target_qualified_name
            WHERE e.edge_kind IN ('calls', 'depends_on', 'type_ref', 'implements')
            ORDER BY e.source_id ASC, target.id ASC, e.edge_kind ASC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(GraphDependencyEdgeRecord {
                source_symbol_id: row.get(0)?,
                target_symbol_id: row.get(1)?,
                edge_kind: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_symbol_edges_for_source_and_kinds(
        &self,
        source_id: &str,
        edge_kinds: &[EdgeKind],
    ) -> Result<Vec<SymbolEdge>, StoreError> {
        let source_id = source_id.trim();
        if source_id.is_empty() || edge_kinds.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = (0..edge_kinds.len())
            .map(|index| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            r#"
            SELECT source_id, target_qualified_name, edge_kind, file_path
            FROM symbol_edges
            WHERE source_id = ?1
              AND edge_kind IN ({placeholders})
            ORDER BY source_id ASC, target_qualified_name ASC, edge_kind ASC, file_path ASC
            "#
        );

        let params = std::iter::once(source_id.to_owned())
            .chain(edge_kinds.iter().map(|kind| kind.as_str().to_owned()))
            .collect::<Vec<_>>();

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut records = Vec::new();
        for row in rows {
            let (source_id, target_qualified_name, edge_kind, file_path) = row?;
            let edge_kind = edge_kind_from_str(edge_kind.as_str()).ok_or_else(|| {
                StoreError::Compatibility(format!(
                    "unsupported edge kind while reading symbol edges: {edge_kind}"
                ))
            })?;
            records.push(SymbolEdge {
                source_id,
                target_qualified_name,
                edge_kind,
                file_path,
            });
        }

        Ok(records)
    }

    pub async fn sync_graph_for_file(
        &self,
        graph_store: &dyn GraphStore,
        file_path: &str,
    ) -> Result<GraphSyncStats, StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(GraphSyncStats {
                resolved_edges: 0,
                unresolved_edges: 0,
            });
        }

        graph_store.delete_edges_for_file(file_path).await?;

        let symbols = self.list_symbols_for_file(file_path)?;
        for symbol in &symbols {
            graph_store.upsert_symbol_node(symbol).await?;
        }

        let (unresolved_edges, resolved) = {
            let conn = self.conn.lock().unwrap();
            let mut unresolved_edges = 0usize;
            let mut unresolved_stmt = conn.prepare(
                r#"
                SELECT e.source_id, e.target_qualified_name, e.edge_kind
                FROM symbol_edges e
                JOIN symbols source ON source.id = e.source_id
                LEFT JOIN symbols target ON target.qualified_name = e.target_qualified_name
                WHERE e.file_path = ?1
                  AND e.edge_kind IN ('calls', 'type_ref', 'implements')
                  AND target.id IS NULL
                ORDER BY e.source_id ASC, e.target_qualified_name ASC, e.edge_kind ASC
                "#,
            )?;
            let unresolved_rows = unresolved_stmt.query_map(params![file_path], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in unresolved_rows {
                let (source_id, target_qualified_name, edge_kind) = row?;
                unresolved_edges += 1;
                tracing::debug!(
                    source_id = %source_id,
                    target_qualified_name = %target_qualified_name,
                    edge_kind = %edge_kind,
                    file_path = %file_path,
                    "unresolved structural edge skipped during graph sync"
                );
            }

            let mut resolved_stmt = conn.prepare(
                r#"
                SELECT e.source_id, target.id, e.edge_kind, e.file_path
                FROM symbol_edges e
                JOIN symbols source ON source.id = e.source_id
                JOIN symbols target ON target.qualified_name = e.target_qualified_name
                WHERE e.file_path = ?1
                  AND e.edge_kind IN ('calls', 'type_ref', 'implements')
                ORDER BY e.source_id ASC, target.id ASC, e.edge_kind ASC
                "#,
            )?;
            let resolved_rows = resolved_stmt.query_map(params![file_path], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut resolved = Vec::new();
            for row in resolved_rows {
                let (source_id, target_id, edge_kind, file_path) = row?;
                let edge_kind = edge_kind_from_str(edge_kind.as_str()).ok_or_else(|| {
                    StoreError::Compatibility(format!(
                        "unsupported edge kind during graph sync: {edge_kind}"
                    ))
                })?;
                resolved.push(ResolvedEdge {
                    source_id,
                    target_id,
                    edge_kind,
                    file_path,
                });
            }
            (unresolved_edges, resolved)
        };

        let mut resolved_edges = 0usize;
        for edge in &resolved {
            resolved_edges += 1;
            graph_store.upsert_edge(edge).await?;
        }

        Ok(GraphSyncStats {
            resolved_edges,
            unresolved_edges,
        })
    }
    pub(crate) fn store_upsert_edges(&self, edges: &[SymbolEdge]) -> Result<(), StoreError> {
        if edges.is_empty() {
            return Ok(());
        }

        let conn = self.conn.lock().unwrap();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT INTO symbol_edges (
                    source_id, target_qualified_name, edge_kind, file_path
                )
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(source_id, target_qualified_name, edge_kind) DO UPDATE SET
                    file_path = excluded.file_path
                "#,
            )?;

            for edge in edges {
                stmt.execute(params![
                    edge.source_id,
                    edge.target_qualified_name,
                    edge.edge_kind.as_str(),
                    edge.file_path,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn store_get_callers(
        &self,
        target_qualified_name: &str,
    ) -> Result<Vec<SymbolEdge>, StoreError> {
        let target_qualified_name = target_qualified_name.trim();
        if target_qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT source_id, target_qualified_name, file_path
            FROM symbol_edges
            WHERE edge_kind = 'calls'
              AND target_qualified_name = ?1
            ORDER BY source_id ASC, target_qualified_name ASC, file_path ASC
            "#,
        )?;

        let rows = stmt.query_map(params![target_qualified_name], |row| {
            Ok(SymbolEdge {
                source_id: row.get(0)?,
                target_qualified_name: row.get(1)?,
                edge_kind: EdgeKind::Calls,
                file_path: row.get(2)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_get_dependencies(
        &self,
        source_id: &str,
    ) -> Result<Vec<SymbolEdge>, StoreError> {
        let source_id = source_id.trim();
        if source_id.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            r#"
            SELECT source_id, target_qualified_name, file_path
            FROM symbol_edges
            WHERE edge_kind = 'depends_on'
              AND source_id = ?1
            ORDER BY source_id ASC, target_qualified_name ASC, file_path ASC
            "#,
        )?;

        let rows = stmt.query_map(params![source_id], |row| {
            Ok(SymbolEdge {
                source_id: row.get(0)?,
                target_qualified_name: row.get(1)?,
                edge_kind: EdgeKind::DependsOn,
                file_path: row.get(2)?,
            })
        })?;

        let records = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }
    pub(crate) fn store_delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM symbol_edges WHERE file_path = ?1",
            params![file_path],
        )?;
        Ok(())
    }
    pub(crate) fn store_has_dependency_between_files(
        &self,
        file_a: &str,
        file_b: &str,
    ) -> Result<bool, StoreError> {
        let file_a = file_a.trim();
        let file_b = file_b.trim();
        if file_a.is_empty() || file_b.is_empty() {
            return Ok(false);
        }

        let exists = self.conn.lock().unwrap().query_row(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM symbol_edges e
                JOIN symbols s_source ON s_source.id = e.source_id
                JOIN symbols s_target ON s_target.qualified_name = e.target_qualified_name
                WHERE e.edge_kind IN ('calls', 'depends_on', 'type_ref', 'implements')
                  AND (
                      (s_source.file_path = ?1 AND s_target.file_path = ?2)
                      OR
                      (s_source.file_path = ?2 AND s_target.file_path = ?1)
                  )
                LIMIT 1
            )
            "#,
            params![file_a, file_b],
            |row| row.get::<_, i64>(0),
        )?;

        Ok(exists != 0)
    }
}
