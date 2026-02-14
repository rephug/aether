use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability};

use super::{GraphStore, ResolvedEdge, StoreError, SymbolRecord};

pub struct CozoGraphStore {
    db: DbInstance,
}

impl CozoGraphStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref();
        let aether_dir = workspace_root.join(".aether");
        let graph_path = aether_dir.join("graph.db");
        fs::create_dir_all(&aether_dir)?;

        let graph_path_str = graph_path.to_string_lossy().to_string();
        let db = DbInstance::new("sled", &graph_path_str, Default::default())
            .map_err(|err| StoreError::Cozo(err.to_string()))?;
        let store = Self { db };
        store.ensure_schema()?;
        Ok(store)
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        self.ensure_relation(
            r#"
            :create symbols {
                symbol_id: String =>
                qualified_name: String,
                name: String,
                kind: String,
                file_path: String,
                language: String,
                signature_fingerprint: String,
                last_seen_at: Int
            }
            "#,
        )?;
        self.ensure_relation(
            r#"
            :create edges {
                source_id: String,
                target_id: String,
                edge_kind: String =>
                file_path: String
            }
            "#,
        )?;
        Ok(())
    }

    fn ensure_relation(&self, script: &str) -> Result<(), StoreError> {
        match self.run_script(script, BTreeMap::new(), ScriptMutability::Mutable) {
            Ok(_) => Ok(()),
            Err(StoreError::Cozo(message))
                if message.contains("relation exists")
                    || message.contains("already exists")
                    || message.contains("conflicts with an existing one")
                    || message.contains("Duplicated")
                    || message.contains("duplicate") =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn run_script(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
        mutability: ScriptMutability,
    ) -> Result<NamedRows, StoreError> {
        self.db
            .run_script(script, params, mutability)
            .map_err(|err| StoreError::Cozo(err.to_string()))
    }

    fn row_to_symbol(row: &[DataValue]) -> Result<SymbolRecord, StoreError> {
        if row.len() < 7 {
            return Err(StoreError::Cozo("invalid symbol row shape".to_owned()));
        }

        let last_seen_at = row[6]
            .get_int()
            .ok_or_else(|| StoreError::Cozo("invalid last_seen_at value".to_owned()))?;

        Ok(SymbolRecord {
            id: row[0]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid symbol id value".to_owned()))?
                .to_owned(),
            file_path: row[1]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid file_path value".to_owned()))?
                .to_owned(),
            language: row[2]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid language value".to_owned()))?
                .to_owned(),
            kind: row[3]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid kind value".to_owned()))?
                .to_owned(),
            qualified_name: row[4]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid qualified_name value".to_owned()))?
                .to_owned(),
            signature_fingerprint: row[5]
                .get_str()
                .ok_or_else(|| StoreError::Cozo("invalid signature_fingerprint value".to_owned()))?
                .to_owned(),
            last_seen_at,
        })
    }

    fn symbol_name(qualified_name: &str) -> &str {
        qualified_name
            .rsplit("::")
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(qualified_name)
    }
}

impl GraphStore for CozoGraphStore {
    fn upsert_symbol_node(&self, symbol: &SymbolRecord) -> Result<(), StoreError> {
        let mut params = BTreeMap::new();
        params.insert("symbol_id".to_owned(), DataValue::from(symbol.id.clone()));
        params.insert(
            "qualified_name".to_owned(),
            DataValue::from(symbol.qualified_name.clone()),
        );
        params.insert(
            "name".to_owned(),
            DataValue::from(Self::symbol_name(&symbol.qualified_name).to_owned()),
        );
        params.insert("kind".to_owned(), DataValue::from(symbol.kind.clone()));
        params.insert(
            "file_path".to_owned(),
            DataValue::from(symbol.file_path.clone()),
        );
        params.insert(
            "language".to_owned(),
            DataValue::from(symbol.language.clone()),
        );
        params.insert(
            "signature_fingerprint".to_owned(),
            DataValue::from(symbol.signature_fingerprint.clone()),
        );
        params.insert(
            "last_seen_at".to_owned(),
            DataValue::from(symbol.last_seen_at),
        );

        self.run_script(
            r#"
            ?[
                symbol_id,
                qualified_name,
                name,
                kind,
                file_path,
                language,
                signature_fingerprint,
                last_seen_at
            ] <- [[
                $symbol_id,
                $qualified_name,
                $name,
                $kind,
                $file_path,
                $language,
                $signature_fingerprint,
                $last_seen_at
            ]]
            :put symbols {
                symbol_id =>
                qualified_name,
                name,
                kind,
                file_path,
                language,
                signature_fingerprint,
                last_seen_at
            }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }

    fn upsert_edge(&self, edge: &ResolvedEdge) -> Result<(), StoreError> {
        let mut params = BTreeMap::new();
        params.insert(
            "source_id".to_owned(),
            DataValue::from(edge.source_id.clone()),
        );
        params.insert(
            "target_id".to_owned(),
            DataValue::from(edge.target_id.clone()),
        );
        params.insert(
            "edge_kind".to_owned(),
            DataValue::from(edge.edge_kind.as_str().to_owned()),
        );
        params.insert(
            "file_path".to_owned(),
            DataValue::from(edge.file_path.clone()),
        );

        self.run_script(
            r#"
            ?[source_id, target_id, edge_kind, file_path] <- [[
                $source_id,
                $target_id,
                $edge_kind,
                $file_path
            ]]
            :put edges { source_id, target_id, edge_kind => file_path }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }

    fn get_callers(&self, qualified_name: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let qualified_name = qualified_name.trim();
        if qualified_name.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "qname".to_owned(),
            DataValue::from(qualified_name.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at
            ] :=
                *edges{source_id: symbol_id, target_id, edge_kind: "calls"},
                *symbols{symbol_id: target_id, qualified_name: $qname},
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .iter()
            .map(|row| Self::row_to_symbol(row.as_slice()))
            .collect()
    }

    fn get_dependencies(&self, symbol_id: &str) -> Result<Vec<SymbolRecord>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "source_id".to_owned(),
            DataValue::from(symbol_id.to_owned()),
        );
        let rows = self.run_script(
            r#"
            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at
            ] :=
                *edges{source_id: $source_id, target_id: symbol_id, edge_kind: "calls"},
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        rows.rows
            .iter()
            .map(|row| Self::row_to_symbol(row.as_slice()))
            .collect()
    }

    fn get_call_chain(
        &self,
        symbol_id: &str,
        depth: u32,
    ) -> Result<Vec<Vec<SymbolRecord>>, StoreError> {
        let symbol_id = symbol_id.trim();
        if symbol_id.is_empty() || depth == 0 {
            return Ok(Vec::new());
        }

        let mut params = BTreeMap::new();
        params.insert("start".to_owned(), DataValue::from(symbol_id.to_owned()));
        params.insert("max_depth".to_owned(), DataValue::from(depth as i64));
        let rows = self.run_script(
            r#"
            reachable[node, depth] :=
                *edges{source_id: $start, target_id: node, edge_kind: "calls"},
                depth = 1
            reachable[node, depth] :=
                reachable[prev, prev_depth],
                prev_depth < $max_depth,
                *edges{source_id: prev, target_id: node, edge_kind: "calls"},
                depth = prev_depth + 1

            ?[
                symbol_id,
                file_path,
                language,
                kind,
                qualified_name,
                signature_fingerprint,
                last_seen_at,
                depth
            ] :=
                reachable[symbol_id, depth],
                *symbols{
                    symbol_id,
                    qualified_name,
                    file_path,
                    language,
                    kind,
                    signature_fingerprint,
                    last_seen_at
                }

            :order depth, qualified_name, symbol_id
            "#,
            params,
            ScriptMutability::Immutable,
        )?;

        let mut levels = Vec::new();
        let mut seen = HashSet::new();
        for row in &rows.rows {
            if row.len() < 8 {
                return Err(StoreError::Cozo("invalid call chain row shape".to_owned()));
            }
            let record = Self::row_to_symbol(&row[..7])?;
            if !seen.insert(record.id.clone()) {
                continue;
            }
            let depth = row[7]
                .get_int()
                .ok_or_else(|| StoreError::Cozo("invalid depth value".to_owned()))?;
            if depth <= 0 {
                continue;
            }
            let depth_idx = depth as usize - 1;
            while levels.len() <= depth_idx {
                levels.push(Vec::new());
            }
            if let Some(level) = levels.get_mut(depth_idx) {
                level.push(record);
            }
        }

        Ok(levels)
    }

    fn delete_edges_for_file(&self, file_path: &str) -> Result<(), StoreError> {
        let file_path = file_path.trim();
        if file_path.is_empty() {
            return Ok(());
        }

        let mut params = BTreeMap::new();
        params.insert(
            "file_path".to_owned(),
            DataValue::from(file_path.to_owned()),
        );
        self.run_script(
            r#"
            ?[source_id, target_id, edge_kind] :=
                *edges{source_id, target_id, edge_kind, file_path: $file_path}

            :rm edges { source_id, target_id, edge_kind }
            "#,
            params,
            ScriptMutability::Mutable,
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{Store, SymbolRecord};
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

    #[test]
    fn cozo_graph_returns_multi_hop_call_chain() {
        let temp = tempdir().expect("tempdir");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        let beta = symbol("sym-beta", "beta");
        let gamma = symbol("sym-gamma", "gamma");
        let delta = symbol("sym-delta", "delta");
        for row in [&alpha, &beta, &gamma, &delta] {
            graph.upsert_symbol_node(row).expect("upsert symbol");
        }

        for edge in [
            ResolvedEdge {
                source_id: alpha.id.clone(),
                target_id: beta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: beta.id.clone(),
                target_id: gamma.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
            ResolvedEdge {
                source_id: gamma.id.clone(),
                target_id: delta.id.clone(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            },
        ] {
            graph.upsert_edge(&edge).expect("upsert edge");
        }

        let chain = graph
            .get_call_chain(&alpha.id, 3)
            .expect("get call chain at depth 3");
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0][0].id, beta.id);
        assert_eq!(chain[1][0].id, gamma.id);
        assert_eq!(chain[2][0].id, delta.id);
    }

    #[test]
    fn unresolved_edges_are_skipped_during_sync() {
        let temp = tempdir().expect("tempdir");
        let store = crate::SqliteStore::open(temp.path()).expect("open sqlite store");
        let graph = CozoGraphStore::open(temp.path()).expect("open cozo graph store");

        let alpha = symbol("sym-alpha", "alpha");
        store.upsert_symbol(alpha.clone()).expect("upsert alpha");
        store
            .upsert_edges(&[SymbolEdge {
                source_id: alpha.id.clone(),
                target_qualified_name: "missing::target".to_owned(),
                edge_kind: EdgeKind::Calls,
                file_path: "src/lib.rs".to_owned(),
            }])
            .expect("upsert unresolved edge");

        let stats = store
            .sync_graph_for_file(&graph, "src/lib.rs")
            .expect("sync graph for file");
        assert_eq!(stats.resolved_edges, 0);
        assert_eq!(stats.unresolved_edges, 1);

        let deps = graph
            .get_dependencies(&alpha.id)
            .expect("query dependencies after unresolved sync");
        assert!(deps.is_empty());
    }
}
